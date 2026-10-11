"""Memory and CPU budgets (the core goal), on release builds. Not part of a
plain `tests/run.py`: run `tests/run.py perf` (builds release first).

Server: memory idle and with 8 bots talking in one channel, CPU during that.
App (Linux build under Xvfb, software OpenGL): signed in with 300 messages
showing, CPU of the UI thread while idle and while 3 others talk in its voice
channel. Exe sizes. Numbers go to target/tests/test_perf/perf.json and are
printed with -v; a failed check shows the number it got.

Needs xvfb, xdotool, libxkbcommon-x11-0 and Mesa (tools/setup.sh installs them).
"""
import asyncio, json, os, subprocess, time

from lib import DIR, ROOT, Server, check, main, note, spawn

REL = os.environ.get("TUFF_PERF_BIN", os.path.join(ROOT, "target", "release"))
TICK = os.sysconf("SC_CLK_TCK")

# Budgets. Measured on 0.11.2 in the cloud workspace (4 cores): server 5.7 MB
# idle, 7.1 MB with 8 talking, ~4.5% CPU; app UI thread ~0.1% idle.
SERVER_IDLE_MB = 12
SERVER_CALL_MB = 16
SERVER_CALL_CPU = 15
APP_IDLE_CPU = 1.0
APP_CALL_CPU = 2.0
APP_HEAP_MB = 90  # includes Mesa's software renderer
APP_EXE_MB = 20
SERVER_EXE_MB = 4


def kb(pid, field):
    for line in open(f"/proc/{pid}/status"):
        if line.startswith(field + ":"):
            return int(line.split()[1])
    return 0


def cpu_seconds(pid, tid=None):
    stat = open(f"/proc/{pid}/task/{tid or pid}/stat").read().rsplit(")", 1)[1].split()
    return (int(stat[11]) + int(stat[12])) / TICK


def cpu_percent(pid, seconds, tid=None):
    a, t = cpu_seconds(pid, tid), time.time()
    time.sleep(seconds)
    return 100 * (cpu_seconds(pid, tid) - a) / (time.time() - t)


def bots(s, names, seconds):
    return [spawn([os.path.join(REL, "tuffcord-bot"), "--server", s.url, "--password", s.invite(), "--name", n,
                   "--channel", "Lounge", "--tone", str(300 + 60 * i), "--seconds", str(seconds)],
                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            for i, n in enumerate(names)]


async def server_budget(r):
    s = Server("server", bin_dir=REL, LOG_LEVEL="info")
    time.sleep(1)
    r["serverIdleMb"] = kb(s.p.pid, "VmRSS") / 1024
    talkers = bots(s, [f"S{i}" for i in range(8)], 20)
    time.sleep(8)
    r["serverCallCpu"] = cpu_percent(s.p.pid, 8)
    r["serverCallMb"] = kb(s.p.pid, "VmRSS") / 1024
    for b in talkers:
        b.wait(60)
    s.stop()
    check(r["serverIdleMb"] < SERVER_IDLE_MB, f"server idle memory {r['serverIdleMb']:.1f} MB < {SERVER_IDLE_MB}")
    check(r["serverCallMb"] < SERVER_CALL_MB,
          f"server memory with 8 talking {r['serverCallMb']:.1f} MB < {SERVER_CALL_MB}")
    check(r["serverCallCpu"] < SERVER_CALL_CPU,
          f"server CPU with 8 talking {r['serverCallCpu']:.1f}% < {SERVER_CALL_CPU}%")


def seed_messages(data_dir):
    os.makedirs(data_dir, exist_ok=True)
    now = int(time.time() * 1000)
    words = "hey did anyone see that clip 😂 lol yeah the boss fight was rough 👍 ok see you tonight 🎮".split()
    msgs = [{"id": f"m{i}", "channel": "general", "author": f"Friend{i % 5}", "authorId": f"u{i % 5}",
             "text": " ".join((words * 2)[i % len(words):][:8]), "ts": now - (300 - i) * 60_000}
            for i in range(300)]
    json.dump({"general": msgs}, open(os.path.join(data_dir, "messages.json"), "w"))


def free_display():
    n = 140
    while os.path.exists(f"/tmp/.X11-unix/X{n}") or os.path.exists(f"/tmp/.X{n}-lock"):
        n += 1
    return f":{n}"


async def in_voice(s, name):
    c, welcome = await s.login("Watcher", "password1")
    await c.close()
    state = (welcome or {}).get("voiceState") or []
    return any(name == m.get("name") for ch in state for m in ch.get("members", []))


async def app_budget(r):
    s = Server("app-server", bin_dir=REL, LOG_LEVEL="info")
    s.stop()
    seed_messages(os.path.join(s.dir, "data"))
    s.start()
    c, welcome = await s.register("Me")
    await c.close()
    await (await s.register("Watcher"))[0].close()
    token = (welcome or {}).get("token", "")
    check(bool(token), "the app's account was created")
    app_dir = os.path.join(DIR, "app")
    os.makedirs(app_dir, exist_ok=True)
    settings = os.path.join(app_dir, "settings.json")
    json.dump({"server": s.url, "name": "Me", "token": token, "remember": True, "auto_connect": True,
               "check_updates": False, "last_text_channel": "general"}, open(settings, "w"))
    display = free_display()
    spawn(["Xvfb", display, "-screen", "0", "1280x800x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1)
    env = dict(os.environ, DISPLAY=display, LIBGL_ALWAYS_SOFTWARE="1", HOME=app_dir, BACKROOM_SETTINGS=settings)
    app = spawn([os.path.join(REL, "TUFFcord")], cwd=app_dir, env=env,
                stdout=open(os.path.join(app_dir, "app.out"), "w"), stderr=subprocess.STDOUT)
    log = os.path.join(app_dir, "TUFFcord.log")
    end = time.time() + 20
    while time.time() < end and not (os.path.exists(log) and "Signed in" in open(log).read()):
        time.sleep(0.3)
    signed_in = os.path.exists(log) and "Signed in" in open(log).read()
    check(signed_in, "the app signed in under Xvfb")
    if not signed_in:
        return
    time.sleep(3)
    r["appIdleCpu"] = cpu_percent(app.pid, 6, app.pid)
    # Lounge in the sidebar, at the default window size and position.
    subprocess.run(["xdotool", "mousemove", "76", "266", "click", "1"], env=env)
    time.sleep(1)
    joined = await in_voice(s, "Me")
    check(joined, "the app joined the voice channel")
    talkers = bots(s, ["T1", "T2", "T3"], 20)
    time.sleep(6)
    r["appCallCpu"] = cpu_percent(app.pid, 10, app.pid)
    r["appCallTotalCpu"] = cpu_percent(app.pid, 2)
    r["appHeapMb"] = kb(app.pid, "RssAnon") / 1024
    for b in talkers:
        b.wait(60)
    check(r["appIdleCpu"] < APP_IDLE_CPU, f"app UI thread idle {r['appIdleCpu']:.2f}% < {APP_IDLE_CPU}%")
    if joined:
        check(r["appCallCpu"] < APP_CALL_CPU,
              f"app UI thread with 3 talking and 300 messages {r['appCallCpu']:.2f}% < {APP_CALL_CPU}%")
    check(r["appHeapMb"] < APP_HEAP_MB, f"app heap {r['appHeapMb']:.0f} MB < {APP_HEAP_MB}")


async def run():
    r = {}
    for name, budget, key in [("TUFFcord", APP_EXE_MB, "appExeMb"),
                              ("TUFFcord-server", SERVER_EXE_MB, "serverExeMb")]:
        r[key] = os.path.getsize(os.path.join(REL, name)) / 1_048_576
        check(r[key] < budget, f"{name} is {r[key]:.1f} MB < {budget}")
    await server_budget(r)
    await app_budget(r)
    with open(os.path.join(DIR, "perf.json"), "w") as f:
        json.dump(r, f, indent=1)
    for k, v in r.items():
        note(f"{k}: {v:.2f}")


main(run)
