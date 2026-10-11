"""Through an HTTPS "tunnel" like Cloudflare's (TLS in front, uploads re-sent
chunked, CF-Connecting-IP added): a video and an image sent by one app arrive
intact at another.

With OLD_BIN set (a folder with the previous release's TUFFcord-server and
tuffcord-bot), also checks old app + new server and new app + old server.
"""
import json, os, struct, subprocess, sys, time, zlib

from lib import DIR, ROOT, Server, binary, check, free_port, main, note, spawn, wait_port

OLD_BIN = os.environ.get("OLD_BIN")


def make_certs(d):
    """A throwaway CA and a certificate for localhost signed by it."""
    def ssl(*a):
        subprocess.run(["openssl", *a], cwd=d, check=True, capture_output=True)
    ssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key", "-out", "ca.crt",
        "-days", "2", "-subj", "/CN=TUFFcord test CA")
    ssl("req", "-newkey", "rsa:2048", "-nodes", "-keyout", "srv.key", "-out", "srv.csr", "-subj", "/CN=localhost")
    open(f"{d}/ext.cnf", "w").write("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\n")
    ssl("x509", "-req", "-in", "srv.csr", "-CA", "ca.crt", "-CAkey", "ca.key", "-CAcreateserial",
        "-out", "srv.crt", "-days", "2", "-extfile", "ext.cnf")
    open(f"{d}/chain.crt", "w").write(open(f"{d}/srv.crt").read() + open(f"{d}/ca.crt").read())
    system = next((p for p in ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt"]
                   if os.path.exists(p)), None)
    open(f"{d}/bundle.pem", "w").write((open(system).read() if system else "") + open(f"{d}/ca.crt").read())
    return f"{d}/bundle.pem"


def png(w, h):
    raw = b"".join(b"\0" + b"".join(bytes((x * 7 % 256, y * 5 % 256, 140)) for x in range(w)) for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def pair(label, app_bin, server_bin, bundle):
    s = Server(label.replace(" ", "-"), bin_dir=server_bin)
    tport = free_port()
    spawn([sys.executable, os.path.join(ROOT, "tests", "tunnel.py"), str(tport), str(s.port), DIR],
          stdout=open(os.path.join(s.dir, "tunnel.out"), "w"), stderr=subprocess.STDOUT)
    wait_port(tport)
    url = f"wss://localhost:{tport}/ws"
    env = dict(os.environ, SSL_CERT_FILE=bundle)
    bot = lambda name, *a, seconds: [binary("tuffcord-bot", app_bin), "--server", url, "--password", s.invite(),
                                     "--name", name, "--seconds", str(seconds), *a]
    recv_out = os.path.join(s.dir, "recv.json")
    recv = spawn(bot("Recv", seconds=10), env=env, stdout=open(recv_out, "w"), stderr=subprocess.STDOUT)
    time.sleep(1.5)
    sent = {}
    for name, path in [("SendVideo", f"{DIR}/video.mp4"), ("SendPic", f"{DIR}/pic.png")]:
        r = subprocess.run(bot(name, "--send-file", path, seconds=3), env=env, capture_output=True, text=True,
                           timeout=90)
        try:
            sent[name] = json.loads(r.stdout)
        except ValueError:
            sent[name] = {"errors": [(r.stdout + r.stderr)[-300:]]}
    recv.wait(30)
    try:
        got = {f["name"]: f for f in json.load(open(recv_out)).get("filesReceived", [])}
    except ValueError:
        got = {}
    for name, file in [("SendVideo", "video.mp4"), ("SendPic", "pic.png")]:
        ok = bool(sent[name].get("fileSent"))
        check(ok, f"{label}: {file} is sent" + ("" if ok else f" ({sent[name].get('errors')})"))
        f = got.get(file)
        check(f is not None and f.get("sizeMatches") and not f.get("error"), f"{label}: {file} arrives intact")
    s.stop()
    refused = [l for l in s.log().splitlines() if "Refused" in l]
    check(not refused, f"{label}: the server refused nothing" + (f" ({refused[0][24:]})" if refused else ""))


async def run():
    bundle = make_certs(DIR)
    open(f"{DIR}/video.mp4", "wb").write(b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41" + os.urandom(6_000_000))
    open(f"{DIR}/pic.png", "wb").write(png(320, 200))
    pair("new app, new server", None, None, bundle)
    if OLD_BIN:
        pair("old app, new server", OLD_BIN, None, bundle)
        pair("new app, old server", None, OLD_BIN, bundle)
    else:
        note("OLD_BIN not set: skipped the old-version pairs")


main(run)
