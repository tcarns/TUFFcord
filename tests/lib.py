"""Shared helpers for the end-to-end tests: start a server on a free port in a
temporary folder, talk to it over WebSocket and HTTP, and record checks.

Each test file calls `lib.main(run)`. Output: one line per failed check, then
a last line `PASS <n> checks` or `FAIL <k> of <n> checks`. Everything else
(server logs, details) goes to the test's own folder, printed at the end only
when something failed or with -v.

Environment:
  TUFF_BIN  folder with TUFFcord-server and tuffcord-bot (default target/debug)
  TUFF_OUT  folder for test output (default target/tests)
"""
import asyncio, atexit, http.client as httpc, json, os, re, shutil, signal, socket, subprocess, sys, time

import websockets

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("TUFF_BIN", os.path.join(ROOT, "target", "debug"))
OUT = os.environ.get("TUFF_OUT", os.path.join(ROOT, "target", "tests"))
VERBOSE = "-v" in sys.argv
NAME = os.path.splitext(os.path.basename(sys.argv[0]))[0]
DIR = os.path.join(OUT, NAME)

_checks = 0
_fails = []
_procs = []


def check(cond, what):
    global _checks
    _checks += 1
    if not cond:
        _fails.append(what)
        print(f"  FAIL {what}", flush=True)
    elif VERBOSE:
        print(f"  ok   {what}", flush=True)


def note(text):
    """Detail kept for -v runs."""
    if VERBOSE:
        print(f"       {text}", flush=True)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def binary(name, bin_dir=None):
    return os.path.abspath(os.path.join(bin_dir or BIN, name))


def spawn(args, **kw):
    """Start a helper process that's ended when the test finishes."""
    p = subprocess.Popen(args, start_new_session=True, **kw)
    _procs.append(p)
    return p


@atexit.register
def _cleanup():
    for p in _procs:
        if p.poll() is None:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except OSError:
                pass


class Server:
    """A server in its own folder (DIR/<name>), on a free port."""

    def __init__(self, name="server", group="g", bin_dir=None, **env):
        self.dir = os.path.join(DIR, name)
        os.makedirs(self.dir, exist_ok=True)
        self.port = free_port()
        self.group = group
        self.exe = binary("TUFFcord-server", bin_dir)
        self.env = dict(os.environ, PASSWORD=group, PORT=str(self.port), CHECK_UPDATES="false",
                        NO_COLOR="1", BACKROOM_NO_WATCHER="1", LOG_LEVEL="debug",
                        DATA_DIR=os.path.join(self.dir, "data"),
                        BACKROOM_CONFIG=os.path.join(self.dir, "none.toml"))
        self.env.update({k: str(v) for k, v in env.items()})
        self.p = None
        self.start()

    @property
    def url(self):
        return f"ws://127.0.0.1:{self.port}/ws"

    @property
    def log_path(self):
        return os.path.join(self.dir, "server.out")

    def start(self):
        self.p = spawn([self.exe], cwd=self.dir, env=self.env, stdin=subprocess.PIPE,
                       stdout=open(self.log_path, "a"), stderr=subprocess.STDOUT, text=True)
        wait_port(self.port)

    def stop(self):
        if self.p and self.p.poll() is None:
            self.p.terminate()
            self.p.wait(10)

    def restart(self):
        self.stop()
        self.start()

    def console(self, line):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()

    def log(self):
        return open(self.log_path).read()

    def invite(self):
        """A fresh one-time invite from the server console. Servers before 0.12
        have no "invite" command: their group password does the same job."""
        if getattr(self, "old", False):
            return self.group
        before = self.log().count("Invite code: ")
        self.console("invite")
        deadline = time.time() + 3
        while time.time() < deadline:
            codes = re.findall(r"Invite code: (\w+)", self.log())
            if len(codes) > before:
                return codes[-1]
            if 'Unknown command "invite"' in self.log():
                self.old = True
                return self.group
            time.sleep(0.02)
        raise RuntimeError("the server made no invite")

    def http(self, method, path, key=None, body=b"", headers=None, chunked=False):
        c = httpc.HTTPConnection("127.0.0.1", self.port, timeout=30)
        h = dict(headers or {})
        if key:
            h["Authorization"] = f"Bearer {key}"
        if chunked:
            parts = [body[i:i + 700_000] for i in range(0, len(body), 700_000)] or [b""]
            c.request(method, path, body=iter(parts), headers=h, encode_chunked=True)
        else:
            c.request(method, path, body=body, headers=h)
        r = c.getresponse()
        return r.status, dict(r.getheaders()), r.read()

    def upload(self, key, name, data, piece=None):
        """Upload in pieces. Returns (status, json) of the last answer."""
        st, _, b = self.http("POST", "/files", key, json.dumps({"name": name, "size": len(data)}).encode(),
                             {"Content-Type": "application/json"})
        if st != 200:
            return st, json.loads(b)
        created = json.loads(b)
        chunk = piece or created["chunk"]
        off, last = 0, None
        while off < len(data):
            part = data[off:off + chunk]
            st, _, b = self.http("PUT", f"/files/{created['id']}?offset={off}", key, part)
            last = (st, json.loads(b))
            if st != 200:
                return last
            off += len(part)
        return 200, {"id": created["id"], **last[1]}

    # Sign-ins -------------------------------------------------------------

    async def connect(self, ip=None, url=None):
        return await Conn.open(url or self.url, ip)

    async def register(self, name, pw="password1", invite=None, ip=None):
        """Create an account, with a console invite unless one is given. An
        invite a refused attempt didn't use is kept for the next one."""
        code = invite if invite is not None else (getattr(self, "spare", None) or self.invite())
        c = await self.connect(ip)
        r = await c.hello(name, code, {"kind": "register", "newPassword": pw})
        if invite is None:
            self.spare = None if r and r.get("type") == "welcome" else code
        return c, r

    async def login(self, name, pw="password1", ip=None):
        c = await self.connect(ip)
        r = await c.hello(name, pw, {"kind": "login"})
        return c, r

    async def token_login(self, token):
        c = await self.connect()
        r = await c.hello("", "", {"kind": "token", "token": token})
        return c, r


def wait_port(port, timeout=10):
    end = time.time() + timeout
    while time.time() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), 0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError(f"nothing listening on port {port}")


class Conn:
    """A WebSocket client that keeps messages it wasn't waiting for."""

    def __init__(self, ws):
        self.ws, self.inbox = ws, []

    @classmethod
    async def open(cls, url, ip=None, ssl=None):
        headers = {"X-Forwarded-For": ip} if ip else None
        ws = await websockets.connect(url, additional_headers=headers, proxy=None, max_size=None, ssl=ssl)
        return cls(ws)

    async def send(self, **m):
        await self.ws.send(json.dumps(m))

    async def hello(self, name, password="", auth=None, accounts=True):
        m = {"type": "hello", "name": name, "password": password, "version": 1, "accounts": accounts}
        if auth:
            m["auth"] = auth
        await self.send(**m)
        return await self.wait("welcome", "error")

    async def wait(self, *types, timeout=4.0):
        end = time.time() + timeout
        while True:
            for i, m in enumerate(self.inbox):
                if m.get("type") in types:
                    return self.inbox.pop(i)
            left = end - time.time()
            if left <= 0:
                return None
            try:
                raw = await asyncio.wait_for(self.ws.recv(), left)
            except (asyncio.TimeoutError, websockets.ConnectionClosed):
                return None
            if isinstance(raw, str):
                self.inbox.append(json.loads(raw))

    async def closed(self, timeout=3.0):
        try:
            await asyncio.wait_for(self.ws.wait_closed(), timeout)
            return True
        except asyncio.TimeoutError:
            return False

    async def close(self):
        await self.ws.close()


def bot(server_url, name, *args, bin_dir=None, password="", seconds=3, env=None):
    """Run tuffcord-bot to completion and return its JSON report."""
    out = subprocess.run([binary("tuffcord-bot", bin_dir), "--server", server_url, "--password", password,
                          "--name", name, "--seconds", str(seconds), *args],
                         capture_output=True, text=True, timeout=seconds + 60, env=env)
    try:
        return json.loads(out.stdout)
    except ValueError:
        return {"ok": False, "error": (out.stdout + out.stderr)[-500:]}


def main(run):
    """Run the test coroutine, then print the summary line and exit."""
    shutil.rmtree(DIR, ignore_errors=True)
    os.makedirs(DIR, exist_ok=True)
    crashed = None
    try:
        asyncio.run(run())
    except Exception as e:  # a broken test counts as a failure, with the reason
        import traceback
        crashed = traceback.format_exc()
        _fails.append(f"test stopped: {type(e).__name__}: {e}")
        print(f"  FAIL test stopped: {type(e).__name__}: {e}", flush=True)
    _cleanup()
    if _fails and (VERBOSE or crashed):
        if crashed:
            print(crashed)
        for root, _, files in os.walk(DIR):
            for f in files:
                if f.endswith(".out"):
                    tail = open(os.path.join(root, f), errors="replace").read().splitlines()[-8:]
                    print(f"  --- last lines of {os.path.relpath(os.path.join(root, f), DIR)}")
                    print("\n".join("  " + l for l in tail))
    if _fails:
        print(f"FAIL {len(_fails)} of {_checks} checks")
        sys.exit(1)
    print(f"PASS {_checks} checks")
