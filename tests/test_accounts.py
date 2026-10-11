"""Accounts: sign-up, sign-in, saved sign-ins, older apps, admins, bans, renames,
password resets, the wrong-password limit, and what's saved to disk."""
import asyncio, time

from lib import Server, check, main, note

GROUP = "letmein"


async def run():
    s = Server(group=GROUP)
    register, login, token_login = s.register, s.login, s.token_login

    note("Creating accounts")
    alice, r = await register("Alice", "alicepass1")
    check(r and r["type"] == "welcome" and r["account"]["name"] == "Alice" and r.get("token"), "Alice creates an account and gets a sign-in token")
    check("accounts" in r.get("features", []), "welcome says the server has accounts")
    alice_token = r["token"]
    _, r = await register("alice", "whatever1")
    check(r and r["code"] == "name_taken", "same name in other case is taken")
    _, r = await register("Mallory", "password1", invite="wrong")
    check(r and r["code"] == "bad_password", "wrong group password refused")
    _, r = await register("Mallory", "1234567")
    check(r and r["code"] == "weak_password", "short password refused")
    _, r = await register("x", "password1")
    check(r and r["code"] == "bad_name", "one-letter name refused")
    _, r = await register("<b>", "password1")
    check(r and r["code"] == "bad_name", "odd characters refused")
    bob, r = await register("  Bob  ", "bobpass11")
    check(r and r["account"]["name"] == "Bob", "names are tidied (spaces trimmed)")
    bob_token = r["token"]

    note("Signing in")
    _, r = await login("ALICE", "alicepass1")
    check(r and r["type"] == "welcome" and r["account"]["name"] == "Alice", "login ignores name case")
    _, r = await login("Alice", "nope")
    check(r and r["code"] == "bad_login", "wrong password refused")
    _, r = await login("Nobody", "nope")
    check(r and r["code"] == "bad_login", "unknown name gives the same answer")
    c, r = await token_login(alice_token)
    check(r and r["type"] == "welcome" and r["account"]["id"] == 1 and "token" not in r, "saved sign-in works (no new token)")
    _, r = await token_login("f" * 64)
    check(r and r["code"] == "session_expired", "made-up sign-in refused")

    note("Older apps")
    c = await s.connect()
    r = await c.hello("Carol", GROUP, accounts=False)
    check(r and r["code"] == "bad_password" and "new TUFFcord" in r["message"], "0.5 app with only the group password can't read the chat; told to get the new app")
    c2 = await s.connect()
    r = await c2.hello("alice", GROUP, accounts=False)
    check(r and r["code"] == "bad_password" and "has one" in r["message"], "0.5 app can't use a name that has an account")
    c = await s.connect()
    r = await c.hello("Carol", GROUP, accounts=True)
    check(r and r["code"] == "account_required", "new app signing in the old way is asked to create an account")
    c = await s.connect()
    r = await c.hello("Alice", "alicepass1", accounts=False)
    check(r and r["type"] == "welcome", "0.5 app works with your own account password")
    c = await s.connect()
    r = await c.hello("Carol", "wrong", accounts=False)
    check(r and r["code"] == "bad_password" and "not right" in r["message"], "0.5 app with a wrong password is refused")

    note("Admins")
    _, r = await alice.send(type="admin", action={"do": "kick", "account": 2}), None
    r = await alice.wait("error")
    check(r and r["code"] == "not_admin", "non-admins can't kick")
    s.console("admin alice")
    r = await alice.wait("accountUpdated")
    check(r and r["account"]["admin"] is True, "console 'admin alice' makes her an admin (she's told)")
    r = await alice.wait("accounts")
    check(r and [a["name"] for a in r["list"]] == ["Alice", "Bob"], "admins get the account list")
    await alice.send(type="admin", action={"do": "setAdmin", "account": 1, "admin": False})
    r = await alice.wait("error")
    check(r and "at least one admin" in r["message"], "the last admin can't be removed")

    note("Owner")
    await alice.send(type="admin", action={"do": "setAdmin", "account": 2, "admin": True})
    r = await alice.wait("adminResult")
    check(r and "now an admin" in r["text"], "the first admin (the owner) can make Bob an admin")
    r = await bob.wait("accountUpdated")
    for act in ({"do": "setAdmin", "account": 1, "admin": False}, {"do": "resetPassword", "account": 1}, {"do": "kick", "account": 1}):
        await bob.send(type="admin", action=act)
        r = await bob.wait("error")
        check(r and r["code"] == "admin_failed" and "owner" in r["message"], f"another admin can't {act['do']} the owner")
    await alice.send(type="admin", action={"do": "setAdmin", "account": 2, "admin": False})
    r = await alice.wait("adminResult")
    check(r and "no longer an admin" in r["text"], "the owner can take admin away")
    await bob.wait("accountUpdated")

    await alice.send(type="admin", action={"do": "kick", "account": 2})
    r = await bob.wait("error")
    check(r and r["code"] == "kicked" and "Alice" in r["message"], "kicked: Bob is told who did it")
    check(await bob.closed(), "kicked: Bob's connection closes")
    r = await alice.wait("adminResult")
    check(r and r["text"] == "Kicked Bob.", "Alice sees the result")
    bob, r = await token_login(bob_token)
    check(r and r["type"] == "welcome", "after a kick Bob can sign back in")

    await alice.send(type="admin", action={"do": "resetPassword", "account": 2})
    r = await alice.wait("adminResult")
    temp = r and r.get("secret")
    check(bool(temp) and len(temp) == 10, "reset password gives Alice a temporary password")
    r = await bob.wait("error")
    check(r and r["code"] == "password_reset", "Bob is disconnected and told")
    _, r = await token_login(bob_token)
    check(r and r["code"] == "session_expired", "Bob's saved sign-in stopped working")
    _, r = await login("Bob", "bobpass11")
    check(r and r["code"] == "bad_login", "Bob's old password stopped working")
    bob, r = await login("Bob", temp)
    check(r and r["type"] == "welcome" and r.get("mustChangePassword") is True, "Bob signs in with the temporary password and must change it")
    await bob.send(type="changePassword", oldPassword="", newPassword="bobnewpass")
    r = await bob.wait("accountUpdated")
    check(r and r.get("token"), "changing it gives Bob a fresh sign-in")
    bob_token = r["token"]
    r = await bob.wait("notice")
    check(r and "Password changed" in r["text"], "Bob is told it worked")
    _, r = await login("Bob", "bobnewpass")
    check(r and r["type"] == "welcome" and not r.get("mustChangePassword"), "the new password works")

    await bob.send(type="changePassword", oldPassword="wrong", newPassword="another1")
    r = await bob.wait("error", timeout=5)
    check(r and r["code"] == "bad_old_password", "changing the password needs the current one")

    note("Renames")
    await bob.send(type="rename", name="Alice")
    r = await bob.wait("error")
    check(r and r["code"] == "bad_rename", "can't take someone else's name")
    await bob.send(type="rename", name="Robert")
    r = await bob.wait("accountUpdated")
    check(r and r["account"]["name"] == "Robert", "rename works")
    r = await alice.wait("presence")
    while r and not any(u["name"] == "Robert" for u in r["users"]):
        r = await alice.wait("presence")
    check(r is not None and any(u["name"] == "Robert" and u.get("account") == 2 for u in r["users"]), "others see the new name, with the same account id")
    _, r = await register("Bob", "bobagain1")
    check(r and r["type"] == "welcome" and r["account"]["id"] == 3, "the old name is free again")

    note("Bans")
    robert, r = await login("Robert", "bobnewpass", ip="203.0.113.7")
    check(r and r["type"] == "welcome", "Robert signs in from 203.0.113.7")
    await alice.send(type="admin", action={"do": "ban", "account": 1})
    r = await alice.wait("error")
    check(r and "yourself" in r["message"], "can't ban yourself")
    await alice.send(type="admin", action={"do": "ban", "account": 2})
    r = await robert.wait("error")
    check(r and r["code"] == "banned", "Robert is banned and told")
    _, r = await login("Robert", "bobnewpass")
    check(r and r["code"] == "banned", "a banned account can't sign in")
    _, r = await register("Rob2", "robpass11", ip="203.0.113.7")
    check(r and r["code"] == "banned", "no new account from the banned address")
    _, r = await register("Rob3", "robpass11", ip="198.51.100.2")
    check(r and r["type"] == "welcome", "other addresses still can")
    await alice.send(type="admin", action={"do": "unban", "account": 2})
    await alice.wait("adminResult")
    _, r = await login("Robert", "bobnewpass")
    check(r and r["type"] == "welcome", "unbanned: Robert can sign in again")

    note("Signing out")
    c, r = await login("Bob", "bobagain1")
    tok = r["token"]
    await c.send(type="signOut")
    await asyncio.sleep(0.3)
    _, r = await token_login(tok)
    check(r and r["code"] == "session_expired", "signing out forgets that device's sign-in")

    note("Too many wrong passwords")
    for i in range(10):
        await login("Alice", "wrong", ip="192.0.2.50")
    _, r = await login("Alice", "alicepass1", ip="192.0.2.50")
    check(r and r["code"] == "too_many_attempts", "after 10 wrong passwords an address has to wait")
    _, r = await login("Alice", "alicepass1", ip="192.0.2.51")
    check(r and r["type"] == "welcome", "other addresses aren't affected")

    note("Guesses sent all at once")
    for _ in range(5):
        await login("Bob", "wrong", ip="192.0.2.60")
    await asyncio.sleep(0.5)  # let those connections close
    rs = await asyncio.gather(*[login("Bob", "wrong", ip="192.0.2.60") for _ in range(8)])
    refused = sum(1 for _, r in rs if r and r["code"] == "too_many_attempts")
    check(refused == 3, f"8 guesses at once after 5 wrong ones: only 5 more are checked ({refused} refused straight away)")
    rs = await asyncio.gather(*[login("Robert", "wrong", ip=f"198.18.0.{i}") for i in range(25)])
    _, r = await login("Robert", "bobnewpass", ip="198.18.1.1")
    check(r and r["code"] == "too_many_attempts", "20 wrong passwords for one name from many addresses: that name has to wait")
    _, r = await login("Robert", "bobnewpass")
    check(r and r["type"] == "welcome", "...except from the address that last signed in to it")

    note("Voice isn't held up by password checks")
    pinger = alice
    rtts = []

    async def ping_loop():
        for _ in range(60):
            t = time.perf_counter()
            await pinger.send(type="ping", t=0)
            await pinger.wait("pong")
            rtts.append((time.perf_counter() - t) * 1000)
            await asyncio.sleep(0.01)

    async def hash_storm():
        await asyncio.gather(*[login("Alice", "alicepass1", ip=f"10.0.0.{i}") for i in range(8)])

    await asyncio.gather(ping_loop(), hash_storm())
    rtts.sort()
    note(f"ping during 8 password checks: median {rtts[len(rtts)//2]:.1f} ms, worst {rtts[-1]:.1f} ms")
    check(rtts[len(rtts) // 2] < 30, "pings stay quick while passwords are checked")

    note("Saved to disk")
    data = open(f"{s.dir}/data/accounts.json").read()
    check("alicepass1" not in data and alice_token not in data and "$argon2id$" in data, "accounts.json has hashes only, no passwords or sign-ins")
    s.restart()
    _, r = await token_login(alice_token)
    check(r and r["type"] == "welcome" and r["account"]["admin"], "after a restart, saved sign-ins and admins are still there")
    s.console("users")
    s.console("resetpw Robert")
    s.console("deluser Rob3")
    await asyncio.sleep(1.0)
    s.stop()
    out = s.log()
    check("Temporary password:" in out and "Deleted Rob3's account" in out, "console resetpw and deluser work")
    check("accounts:" in out and "Rob3" in out, "console 'users' lists accounts")



main(run)
