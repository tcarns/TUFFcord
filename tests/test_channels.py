"""Admins adding, renaming, deleting and reordering channels: only admins,
names are cleaned, duplicates refused, everyone gets the new lists, messages
and people in voice move with a rename, and it all survives a restart."""
import asyncio, os

from lib import Server, check, main


async def drain(conn, kind):
    """Drop queued messages of one kind, so the next wait sees a fresh one."""
    while await conn.wait(kind, timeout=0.3):
        pass


async def run():
    s = Server()
    alice, w = await s.register("Alice")
    check("channels" in w.get("features", []), "the server says it can add channels")
    s.console("admin Alice")
    await alice.wait("accountUpdated")
    bob, _ = await s.register("Bob")

    await bob.send(type="createChannel", name="nope")
    e = await bob.wait("error")
    check(e and e["code"] == "channel_failed", "non-admins are refused")
    check(await alice.wait("channels", timeout=0.7) is None, "...and nothing is added")

    await alice.send(type="createChannel", name="  Movie Night! ")
    ca, cb = await alice.wait("channels"), await bob.wait("channels")
    check(ca and ca["textChannels"][-1] == "movie-night", "text channel names are cleaned")
    check(cb and cb["textChannels"] == ca["textChannels"], "everyone gets the new list")

    await alice.send(type="createChannel", name="Movie-night")
    e = await alice.wait("error")
    check(e and e["code"] == "channel_failed", "a name that's taken is refused")

    await alice.send(type="createChannel", name="Late Talk", voice=True)
    c = await bob.wait("channels")
    await alice.wait("channels")
    check(c and c["voiceChannels"][-1] == "Late Talk", "voice channels keep their name")
    await bob.send(type="joinVoice", channel="Late Talk")
    j = await bob.wait("voiceJoined")
    check(j and j["channel"] == "Late Talk", "the new voice channel can be joined")
    await alice.send(type="chat", channel="movie-night", text="hi")
    m = await bob.wait("chat")
    check(m and m["message"]["channel"] == "movie-night", "the new text channel works")

    await alice.wait("chat")

    # Rename, delete, reorder (0.9.2).
    check("channelEdit" in w.get("features", []), "the server says channels can be edited")
    await bob.send(type="renameChannel", name="movie-night", to="nope")
    e = await bob.wait("error")
    check(e and e["code"] == "channel_failed", "non-admins can't rename")
    await bob.send(type="deleteChannel", name="movie-night")
    e = await bob.wait("error")
    check(e and e["code"] == "channel_failed", "non-admins can't delete")
    await bob.send(type="moveChannel", name="movie-night", index=0)
    e = await bob.wait("error")
    check(e and e["code"] == "channel_failed", "non-admins can't reorder")
    await bob.wait("channels")  # the server re-sends the real order

    await alice.send(type="renameChannel", name="movie-night", to="Film Club")
    ca, cb = await alice.wait("channels"), await bob.wait("channels")
    check(ca and "film-club" in ca["textChannels"] and "movie-night" not in ca["textChannels"],
          "a text channel can be renamed (name cleaned)")
    check(cb and cb.get("renamed") == {"from": "movie-night", "to": "film-club", "voice": False},
          "everyone is told the old and new name")
    await alice.send(type="renameChannel", name="film-club", to="general")
    e = await alice.wait("error")
    check(e and e["code"] == "channel_failed", "renaming to a name that's taken is refused")

    await drain(bob, "voiceState")
    await alice.send(type="renameChannel", name="Late Talk", to="Night Talk", voice=True)
    cb = await bob.wait("channels")
    check(cb and "Night Talk" in cb["voiceChannels"], "a voice channel can be renamed")
    j = await bob.wait("voiceJoined")
    check(j and j["channel"] == "Night Talk", "people in it are told they're in the new name")
    v = await bob.wait("voiceState")
    here = [c for c in (v or {}).get("state", []) if c["name"] == "Night Talk"]
    check(here and [m["name"] for m in here[0]["members"]] == ["Bob"], "...and are still in voice")

    await alice.send(type="moveChannel", name="film-club", index=0)
    ca, cb = await alice.wait("channels"), await bob.wait("channels")
    check(cb and cb["textChannels"][0] == "film-club", "channels can be reordered")

    await alice.send(type="createChannel", name="temp")
    await alice.wait("channels"), await bob.wait("channels")
    await alice.send(type="chat", channel="temp", text="keep me")
    await bob.wait("chat"), await alice.wait("chat")
    _, up = s.upload(w["fileKey"], "clip.bin", os.urandom(3000))
    await alice.send(type="post", channel="temp", text="a file", files=[{"upload": up["id"], "width": 0, "height": 0}])
    await bob.wait("chat"), await alice.wait("chat")
    await alice.send(type="deleteChannel", name="temp")
    ca, cb = await alice.wait("channels"), await bob.wait("channels")
    check(cb and "temp" not in cb["textChannels"], "a text channel can be deleted")
    status, _, _ = s.http("GET", f"/files/{up['id']}", key=w["fileKey"])
    check(status == 404, "...its files are gone from the server")
    trash = os.path.join(s.dir, "data", "trash")
    check(os.path.isdir(trash) and len(os.listdir(trash)) == 1, "...but kept in the trash")

    # The server window brings it back (fix for a stolen admin account).
    s.console("restore 1")
    cb = await bob.wait("channels")
    check(cb and "temp" in cb["textChannels"], "a deleted channel can be restored from the server window")
    await bob.send(type="loadOlder", channel="temp")
    o = await bob.wait("olderMessages")
    check(o and [m["text"] for m in o["messages"]] == ["keep me", "a file"], "...with its messages")
    status, _, _ = s.http("GET", f"/files/{up['id']}", key=w["fileKey"])
    check(status == 200, "...and its files")
    check(not os.listdir(trash), "...and it leaves the trash")
    await alice.wait("channels")
    await alice.send(type="deleteChannel", name="temp")
    await alice.wait("channels"), await bob.wait("channels")

    await drain(bob, "voiceState")
    await alice.send(type="deleteChannel", name="Night Talk", voice=True)
    cb = await bob.wait("channels")
    check(cb and "Night Talk" not in cb["voiceChannels"], "a voice channel can be deleted")
    v = await bob.wait("voiceState")
    check(v and all(m["name"] != "Bob" for c in v["state"] for m in c["members"]),
          "people in a deleted voice channel are moved out of voice")
    for ch in cb["voiceChannels"][1:]:
        await alice.send(type="deleteChannel", name=ch, voice=True)
        await alice.wait("channels")
    await alice.send(type="deleteChannel", name=cb["voiceChannels"][0], voice=True)
    e = await alice.wait("error")
    check(e and e["code"] == "channel_failed", "the last channel of a kind can't be deleted")

    await asyncio.sleep(1.5)  # history is saved once a second
    s.restart()
    _, w = await s.register("Carl")
    check(w["textChannels"][0] == "film-club" and "temp" not in w["textChannels"],
          "renames, order and deletions are still there after a restart")
    check(len(w["voiceChannels"]) == 1 and "Night Talk" not in w["voiceChannels"],
          "...for voice channels too")
    hist = [m["text"] for m in w["history"].get("film-club", [])]
    check(hist == ["hi"] and "movie-night" not in w["history"], "messages moved with the rename")
    s.stop()
    log = s.log()
    check("added the text channel #movie-night" in log, "the server log says who added it")
    check("restored the text channel #temp (2 message(s), 1 file(s))" in log, "the restore is in the admin log")
    check("renamed the text channel #movie-night to #film-club" in log
          and "deleted the voice channel Night Talk" in log, "...and who renamed and deleted")


main(run)
