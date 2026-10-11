"""Console invites, commands for a running server, signed-in devices, new-address notices, the admin log, and
look-alike names."""
import asyncio, json, os, re, subprocess, time

from lib import Server, check, main, note

GROUP = "letmein"


async def run():
    s2 = Server(name="open", group=GROUP)

    note("Invites")
    _, r = await s2.register("Owner", "ownerpass1", invite=GROUP)
    check(r and r["code"] == "bad_password", "the old group password can't create an account")
    _, r = await s2.register("Owner", "ownerpass1", invite="")
    check(r and r["code"] == "bad_password", "...nor can no invite")
    owner, r = await s2.register("Owner", "ownerpass1", ip="198.51.100.1")
    check(r and r["type"] == "welcome" and {"sessions", "adminLog"} <= set(r["features"]) and "invites" not in r["features"],
          "a console invite creates an account; welcome lists sessions and adminLog, not app invites")
    owner_token = r["token"]
    s2.console("admin Owner")
    await owner.wait("accountUpdated")
    await owner.send(type="createInvite")
    r = await owner.wait("error")
    check(r and r["code"] == "invite_failed" and "server window" in r["message"], "admins can't make invites in the app (told to use the server window)")
    code = s2.invite()
    check(len(code) == 10, "the console prints a 10-character invite code")
    accounts = json.load(open(os.path.join(s2.dir, "data", "accounts.json")))
    left = max(i["expires"] for i in accounts["invites"]) / 1000 - time.time()
    check(23.9 * 3600 < left <= 24 * 3600, "invites last 24 hours")
    mallory, _ = await s2.register("Mallory", "mallorypass")
    _, r = await s2.register("Guest", "guestpass1", invite=code)
    check(r and r["type"] == "welcome", "the invite creates an account")
    _, r = await s2.register("Guest2", "guestpass1", invite=code)
    check(r and r["code"] == "bad_password", "...once only")
    for _ in range(11):
        s2.console("invite")
    time.sleep(0.5)
    check(s2.log().count("Invite code: ") == 13 and "already 10 unused invites" in s2.log(), "at most 10 unused invites at once")

    note("Commands from the command line")
    out = subprocess.run([s2.exe, "status"], env=s2.env, capture_output=True, text=True, timeout=10)
    check(out.returncode == 0 and "Owner" in out.stdout, "TUFFcord-server status answers from the running server")
    out = subprocess.run([s2.exe, "invite"], env=s2.env, capture_output=True, text=True, timeout=10)
    check(out.returncode == 0 and "already 10" in out.stdout, "...and invite too (still capped)")
    mode = os.stat(os.path.join(s2.dir, "data", "console.sock")).st_mode & 0o777
    check(mode == 0o600, "only the server's own user can send commands")
    cmd = lambda *a: subprocess.run([s2.exe, *a], env=s2.env, capture_output=True, text=True, timeout=10).stdout
    out = cmd("invites")
    check(out.startswith("10 unused") and "10. made" in out and "h " in out and code not in out,
          "invites lists the 10 unused ones with time left, without their codes")
    check("Cancelled an invite. 9 left." in cmd("uninvite", "1") and cmd("invites").startswith("9 unused"), "uninvite <n> cancels one")
    check("uninvite <n>" in cmd("uninvite", "42"), "...a wrong number is explained")
    check("Cancelled 9 invites" in cmd("uninvite", "all") and "No unused invites" in cmd("invites"), "uninvite all cancels the rest")

    note("Look-alike names")
    cancelled = re.findall(r"Invite code: (\w+)", s2.log())[-1]
    _, r = await s2.register("Someone", "somepass1", invite=cancelled)
    check(r and r["code"] == "bad_password", "a cancelled invite no longer works")
    _, r = await s2.register("Ownеr", "ownerpass1")
    check(r and r["code"] == "bad_name", "a name mixing Latin and Cyrillic letters is refused")

    note("Signed-in devices")
    laptop, r = await s2.login("Owner", "ownerpass1", ip="198.51.100.1")
    laptop_token = r["token"]
    await owner.send(type="sessions")
    r = await owner.wait("sessions")
    check(r and len(r["list"]) == 2 and sum(x["current"] for x in r["list"]) == 1, "sessions lists both devices, this one marked")
    check(all(x["ip"] == "198.51.100.1" for x in r["list"]), "...with the address each was last used from")

    note("New-address notice")
    phone, r = await s2.login("Owner", "ownerpass1", ip="203.0.113.9")
    n = await owner.wait("notice")
    check(n and "new address (203.0.113.9)" in n["text"], "a sign-in from a new address is shown on the other devices")
    await laptop.wait("notice")
    await laptop.send(type="endSession")
    n = await laptop.wait("notice")
    check(n and "Signed out 2 other devices" in n["text"], "'sign out everywhere else' signs out the other two")
    r = await owner.wait("error")
    check(r and r["code"] == "session_expired", "...and they're disconnected")
    _, r = await s2.token_login(owner_token)
    check(r and r["code"] == "session_expired", "...and their saved sign-ins stop working")
    again, r = await s2.token_login(laptop_token)
    check(r and r["type"] == "welcome", "this device stays signed in")
    await again.ws.close()
    await laptop.send(type="sessions")
    r = await laptop.wait("sessions")
    one = [x for x in r["list"] if not x["current"]]
    check(r and not one, "only this device is left")

    # The notice waits when no other device is online.
    await laptop.ws.close()
    await asyncio.sleep(0.3)
    await s2.login("Owner", "ownerpass1", ip="192.0.2.77")
    later, r = await s2.token_login(laptop_token)
    n = await later.wait("notice")
    check(n and "192.0.2.77" in n["text"], "nobody online: the notice shows on the next device to connect")

    note("Admin log")
    await later.send(type="admin", action={"do": "kick", "account": 2})
    await later.wait("adminResult", "error")
    await later.send(type="adminLog")
    r = await later.wait("adminLog")
    texts = [e["text"] for e in (r or {}).get("entries", [])]
    check(texts[:1] == ["Owner kicked Mallory"] and "The server console made an invite" in texts, "admins see recent admin actions, newest first")
    mallory, _ = await s2.login("Mallory", "mallorypass")
    await mallory.send(type="adminLog")
    r = await mallory.wait("adminLog", timeout=1)
    check(r is None, "non-admins get nothing")
    s2.restart()
    c, r = await s2.token_login(laptop_token)
    await c.send(type="adminLog")
    r = await c.wait("adminLog")
    check(r and len(r["entries"]) >= 2, "the admin log survives a restart")
    s2.stop()


main(run)
