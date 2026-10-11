"""Files over HTTP: uploads in pieces, posting, downloads with Range, keys,
the storage limit, older apps, and what survives a restart."""
import asyncio, json, os

from lib import Server, check, main, note

GROUP = "letmein"


async def run():
    s = Server(group=GROUP, MAX_ATTACHMENT_MB=3, MAX_STORAGE_MB=5)
    http, upload = s.http, s.upload
    note("Signing in")
    alice, w = await s.register("Alice", "secret12")
    check(w and w["type"] == "welcome" and w.get("fileKey") and "files" in w["features"], "welcome has a file key and the files feature")
    check(w and [m["name"] for m in w.get("members", [])] == ["Alice"], "welcome lists members")
    key = w["fileKey"]
    bob, wb = await s.register("Bob", "secret12")
    m = await alice.wait("members")
    check(m and [x["name"] for x in m["list"]] == ["Alice", "Bob"], "everyone hears about a new member")
    bob_key = wb["fileKey"]

    note("Uploading")
    st, _, b = http("GET", "/")
    check(st == 200 and b.startswith(b"TUFFcord server"), "GET / answers")
    st, _, b = http("POST", "/files", None, b'{"name":"x","size":1}')
    check(st == 401, "no key, no upload")
    st, _, b = http("POST", "/files", "nonsense", b'{"name":"x","size":1}')
    check(st == 401, "made-up key refused")
    st, r = upload(key, "huge.bin", b"x" * (3 * 1024 * 1024 + 1))
    check(st == 413 and r["code"] == "too_big", "over the size limit refused before uploading")

    mp4 = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41" + os.urandom(2_500_000)
    st, r = upload(key, "C:\\clips\\funny <clip>.mp4", mp4, piece=1_000_000)
    check(st == 200 and r["done"] and r["received"] == len(mp4), "a 2.5 MB video uploads in pieces")
    vid = r["id"]
    jpg = b"\xff\xd8\xff\xe0" + os.urandom(20_000)
    st, r = upload(key, "poster.jpg", jpg)
    poster = r["id"]
    st, r2 = upload(key, "notes.pdf", b"%PDF-1.7\n" + os.urandom(5000))
    pdf = r2["id"]

    # Out-of-order piece is answered with where to carry on.
    st0, _, b = http("POST", "/files", key, json.dumps({"name": "a.txt", "size": 10}).encode())
    aid = json.loads(b)["id"]
    st, _, b = http("PUT", f"/files/{aid}?offset=5", key, b"12345")
    check(st == 409 and json.loads(b)["received"] == 0, "a piece in the wrong place gets 409 + where to resume")
    st, _, b = http("PUT", f"/files/{aid}?offset=0", bob_key, b"12345")
    check(st == 403, "someone else can't add to your upload")
    st, _, b = http("PUT", f"/files/{aid}?offset=0", None, b"12345")
    check(st == 401, "no key: a piece is refused before its body is read")

    note("Posting")
    await bob.ws.send(json.dumps({"type": "post", "channel": "general", "text": "look",
                                  "files": [{"upload": vid, "width": 1920, "height": 1080, "durationMs": 7400, "poster": poster}]}))
    e = await bob.wait("error")
    check(e and e["code"] == "post_failed", "can't post someone else's upload")
    await alice.ws.send(json.dumps({"type": "post", "channel": "general", "text": "look",
                                    "files": [{"upload": vid, "width": 1920, "height": 1080, "durationMs": 7400, "poster": poster},
                                              {"upload": pdf, "width": 50, "height": 50}]}))
    m = await bob.wait("chat")
    a = m and m["message"]["attachments"]
    check(a and a[0]["mime"] == "video/mp4" and a[0]["name"] == "funny _clip_.mp4" and a[0]["durationMs"] == 7400
          and a[0]["poster"]["id"] == poster, "video posted with type found by the server, clean name, duration, poster")
    check(a and a[1]["mime"] == "application/pdf" and a[1]["width"] == 0, "pdf posted as a plain file (no size for non-visual)")
    await alice.ws.send(json.dumps({"type": "post", "channel": "general", "files": [{"upload": vid}]}))
    e = await alice.wait("error")
    check(e and e["code"] == "post_failed", "an upload can only be posted once")

    note("Downloading")
    st, h, b = http("GET", f"/files/{vid}", bob_key)
    check(st == 200 and b == mp4 and h.get("Content-Type") == "video/mp4", "Bob downloads the video intact")
    st, h, b = http("GET", f"/files/{vid}", bob_key, headers={"Range": "bytes=1000000-"})
    check(st == 206 and b == mp4[1_000_000:] and h.get("Content-Range") == f"bytes 1000000-{len(mp4)-1}/{len(mp4)}", "Range resumes")
    st, h, b = http("GET", f"/files/{poster}", bob_key)
    check(st == 200 and b == jpg, "the poster downloads")
    st, h, b = http("GET", f"/files/{vid}", None)
    check(st == 401, "no key, no download")
    st, h, b = http("GET", f"/files/{aid}", key)
    check(st == 404, "unposted uploads can't be downloaded")
    st, h, b = http("GET", "/files/..%2f..%2fTUFFcord.log", key)
    check(st == 404, "no path tricks")

    note("Older apps")
    await bob.ws.send(json.dumps({"type": "getAttachment", "id": vid}))
    g = await bob.wait("attachmentGone")
    check(g is not None, "videos aren't sent over the voice connection to older apps")

    note("Storage limit (5 MB)")
    big = b"\0\0\0\x20ftypisom" + os.urandom(2_900_000)
    st, r = upload(key, "second.mp4", big)
    await alice.ws.send(json.dumps({"type": "post", "channel": "general", "files": [{"upload": r["id"]}]}))
    await bob.wait("chat")
    gone = await bob.wait("attachmentGone")
    check(gone and gone["id"] in (vid, poster, pdf), "the oldest files are deleted to make room")
    st, h, b = http("GET", f"/files/{vid}", bob_key)
    check(st == 404, "an expired file can't be downloaded")
    att_dir = f"{s.dir}/data/attachments"
    used = sum(os.path.getsize(os.path.join(att_dir, f)) for f in os.listdir(att_dir))
    check(used <= 5 * 1024 * 1024, f"stored files fit the limit ({used} bytes)")

    note("After a restart")
    await asyncio.sleep(1.5)  # history is saved once a second
    s.restart()
    _, w = await s.login("Alice", "secret12")
    atts = [a for m in w["history"]["general"] for a in m.get("attachments", [])]
    check(any(a.get("expired") for a in atts) and any(not a.get("expired") for a in atts), "expired marks survive a restart")
    leftovers = [f for f in os.listdir(att_dir) if f.endswith(".part") or f == aid]
    check(not leftovers, "unfinished uploads are cleaned up at start")
    s.stop()

    note("Daily limit per person")
    d = Server(name="daily", group=GROUP, MAX_UPLOAD_MB_PER_DAY=1)
    _, w = await d.register("Dana", "secret12")
    st, r = d.upload(w["fileKey"], "a.bin", os.urandom(600_000))
    check(st == 200 and r["done"], "under the daily limit: uploads")
    st, r = d.upload(w["fileKey"], "b.bin", os.urandom(600_000))
    check(st == 429 and r["code"] == "daily_limit", "over it: refused until tomorrow")
    d.stop()



main(run)
