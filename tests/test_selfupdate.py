"""Server self-update on Linux, through the watcher: a copy still named
backroom-server takes its new name when started, then it finds a newer release on a
local web server, refuses it while it isn't signed with the release key, checks
and installs it, restarts (people reconnect), refuses a signed release under
another version, and Ctrl+C stops watcher and server together."""
import asyncio, base64, hashlib, json, os, shutil, signal, struct, subprocess, sys, time

from lib import DIR, Server, binary, check, free_port, main, spawn, wait_port

ASSET = "TUFFcord-server"


def write_release(rel, port, version):
    names = [ASSET, "SHA256SUMS.txt", "SHA256SUMS.txt.sshsig"]
    json.dump({"tag_name": f"v{version}", "html_url": "x", "body": "test", "draft": False, "prerelease": False,
               "assets": [{"name": n, "browser_download_url": f"http://127.0.0.1:{port}/{n}", "size": 1}
                          for n in names]},
              open(os.path.join(rel, "release.json"), "w"))


def _s(b):
    return struct.pack(">I", len(b)) + b


def _pub_blob(path):
    der = subprocess.run(["openssl", "pkey", "-in", path, "-pubout", "-outform", "DER"],
                         check=True, capture_output=True).stdout
    return _s(b"ssh-ed25519") + _s(der[-32:])


def sign(rel, key):
    """Sign the release's checksums with `key` (a PEM file) the way
    `ssh-keygen -Y sign -n tuffcord-release` does (the owner's signing script)."""
    sums = open(os.path.join(rel, "SHA256SUMS.txt"), "rb").read()
    ns = b"tuffcord-release"
    signed = b"SSHSIG" + _s(ns) + _s(b"") + _s(b"sha512") + _s(hashlib.sha512(sums).digest())
    data = os.path.join(rel, "signed.bin")
    open(data, "wb").write(signed)
    raw = subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", key, "-rawin", "-in", data],
                         check=True, capture_output=True).stdout
    blob = (b"SSHSIG" + struct.pack(">I", 1) + _s(_pub_blob(key)) + _s(ns) + _s(b"") + _s(b"sha512")
            + _s(_s(b"ssh-ed25519") + _s(raw)))
    b = base64.b64encode(blob).decode()
    lines = [b[i:i + 70] for i in range(0, len(b), 70)]
    open(os.path.join(rel, "SHA256SUMS.txt.sshsig"), "w").write(
        "-----BEGIN SSH SIGNATURE-----\n" + "\n".join(lines) + "\n-----END SSH SIGNATURE-----\n")


def new_key(path):
    """A new Ed25519 key; returns its public half as a .pub line."""
    subprocess.run(["openssl", "genpkey", "-algorithm", "ed25519", "-out", path], check=True)
    return "ssh-ed25519 " + base64.b64encode(_pub_blob(path)).decode() + " test"


async def wait_log(path, text, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        if text in open(path).read():
            return True
        await asyncio.sleep(0.2)
    return False


async def run():
    srv_dir, rel = os.path.join(DIR, "srv"), os.path.join(DIR, "rel")
    os.makedirs(srv_dir), os.makedirs(rel)
    exe = os.path.join(srv_dir, ASSET)
    old_exe = os.path.join(srv_dir, "backroom-server")  # as installed before the rename
    shutil.copy(binary("TUFFcord-server"), old_exe)
    # The "new version": the same program with a few extra bytes, so it's
    # recognizably a different file once installed.
    new = open(binary("TUFFcord-server"), "rb").read() + b"\0new-version"
    open(os.path.join(rel, ASSET), "wb").write(new)
    open(os.path.join(rel, "SHA256SUMS.txt"), "w").write(
        f"version 99.0.0\n{hashlib.sha256(new).hexdigest()}  {ASSET}\n")
    os.chmod(os.path.join(rel, ASSET), 0o755)
    key, other = os.path.join(DIR, "release.pem"), os.path.join(DIR, "other.pem")
    pub = new_key(key)
    new_key(other)
    sign(rel, other)  # signed, but not with the release key
    http_port = free_port()
    spawn([sys.executable, "-m", "http.server", str(http_port), "--bind", "127.0.0.1"], cwd=rel,
          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_port(http_port)
    write_release(rel, http_port, "99.0.0")

    port = free_port()
    out = os.path.join(srv_dir, "server.out")
    env = dict(os.environ, PASSWORD="g", PORT=str(port), NO_COLOR="1", LOG_LEVEL="debug",
               CHECK_UPDATES="true", BACKROOM_UPDATE_URL=f"http://127.0.0.1:{http_port}/release.json",
               BACKROOM_UPDATE_KEY=pub, BACKROOM_SERVER_UPDATE_ASSET=ASSET, BACKROOM_TEST_REPORT_VERSION="99.0.0",
               DATA_DIR=os.path.join(srv_dir, "data"), BACKROOM_CONFIG=os.path.join(srv_dir, "none.toml"))
    env.pop("BACKROOM_NO_WATCHER", None)
    watcher = spawn([old_exe], cwd=srv_dir, env=env, stdin=subprocess.PIPE, stdout=open(out, "w"),
                    stderr=subprocess.STDOUT, text=True)
    wait_port(port)
    check(os.path.exists(exe) and not os.path.exists(old_exe), "the old program name becomes TUFFcord-server")
    s = Server.__new__(Server)  # just for its sign-in helpers, pointed at this server
    s.port, s.group = port, "g"
    alice, w = await s.register("Alice")
    check(w and w["type"] == "welcome", "someone is signed in before the update")

    # It finds 99.0.0 a few seconds after starting; "update" installs and restarts now.
    check(await wait_log(out, "99.0.0"), "the server notices the new release")
    watcher.stdin.write("update\n"), watcher.stdin.flush()
    check(await wait_log(out, "isn't signed with TUFFcord's release key"), "a release signed with another key is refused")
    check(open(exe, "rb").read() != new, "...and nothing is installed")
    sign(rel, key)
    watcher.stdin.write("update\n"), watcher.stdin.flush()
    r = await alice.wait("restarting", timeout=20)
    check(r is not None, "people are told the server is restarting")
    write_release(rel, http_port, "0.0.1")  # stop it updating again after the restart
    check(await alice.closed(10), "the connection closes for the restart")
    wait_port(port, 20)
    _, w = await s.login("Alice")
    check(w and w["type"] == "welcome", "signing back in works after the restart")
    check(open(exe, "rb").read() == new, "the new version is installed in place")
    check(await wait_log(out, "Restarted to finish updating"), "the restarted server says why it restarted")

    # A signed release passed off as another version is refused (its
    # checksums name the version they were signed for).
    write_release(rel, http_port, "98.0.0")
    watcher.stdin.write("update\n"), watcher.stdin.flush()
    check(await wait_log(out, "TUFFcord 98.0.0 is out, but it isn't signed"), "a signed release under another version is refused")
    check(open(exe, "rb").read() == new, "...and nothing is changed")

    # Ctrl+C on the watcher stops both.
    os.killpg(watcher.pid, signal.SIGINT)
    try:
        watcher.wait(10)
    except subprocess.TimeoutExpired:
        pass
    left = subprocess.run(["pgrep", "-f", exe], capture_output=True, text=True).stdout.split()
    check(watcher.poll() is not None and not left, "Ctrl+C stops the watcher and the server")


main(run)
