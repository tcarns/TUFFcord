"""Invites, signed-in devices, new-address notices, the admin log, and
look-alike names."""
import asyncio

from lib import Server, check, main, note

GROUP = "letmein"


async def run():
    s = Server(group=GROUP, ALLOW_SIGNUP="false")

    note("Invites")
    _, r = await s.register("Owner", "ownerpass1")
    check(r and r["code"] == "signup_closed", "with allow_signup off the group password can't create accounts")
    s2 = Server(name="open", group=GROUP)
    owner, r = await s2.register("Owner", "ownerpass1", ip="198.51.100.1")
    check(r and r["type"] == "welcome" and {"invites", "sessions", "adminLog"} <= set(r["features"]), "welcome lists invites, sessions and adminLog")
    owner_token = r["token"]
    s2.console("admin Owner")
    await owner.wait("accountUpdated")
    await owner.send(type="createInvite")
    r = await owner.wait("invite")
    check(r and len(r["code"]) == 10 and r["expires"] > 0, "an admin gets a one-time invite code")
    code = r["code"]
    mallory, _ = await s2.register("Mallory", "mallorypass")
    await mallory.send(type="createInvite")
    r = await mallory.wait("error")
    check(r and r["code"] == "invite_failed", "non-admins can't make invites")
    _, r = await s2.register("Guest", "guestpass1", invite=code)
    check(r and r["type"] == "welcome", "the invite creates an account in place of the group password")
    _, r = await s2.register("Guest2", "guestpass1", invite=code)
    check(r and r["code"] == "bad_password", "...once only")

    note("Look-alike names")
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
    check(texts[:1] == ["Owner kicked Mallory"] and "Owner made an invite" in texts, "admins see recent admin actions, newest first")
    mallory, _ = await s2.login("Mallory", "mallorypass")
    await mallory.send(type="adminLog")
    r = await mallory.wait("adminLog", timeout=1)
    check(r is None, "non-admins get nothing")
    s2.restart()
    c, r = await s2.token_login(laptop_token)
    await c.send(type="adminLog")
    r = await c.wait("adminLog")
    check(r and len(r["entries"]) >= 2, "the admin log survives a restart")
    s.stop(), s2.stop()


main(run)
