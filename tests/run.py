#!/usr/bin/env python3
"""Build the server and test bot, then run the end-to-end suites.

  tests/run.py                 everything (except old/new version pairs and perf)
  tests/run.py perf            memory and CPU budgets (release build, Xvfb)
  tests/run.py files delete    just these suites (test_files.py, test_delete.py)
  tests/run.py --compat        also old app/new server and new app/old server,
                               against the previous release (built once, cached)
  tests/run.py --no-build      use the binaries already in target/debug
  tests/run.py -v              show every check, not just failures

Prints one line per suite, and the failed checks under it. Logs stay in
target/tests/<suite>/. Exits non-zero if anything failed.
Needs: python3 with `websockets`, openssl.
"""
import glob, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
args = [a for a in sys.argv[1:] if not a.startswith("-")]
flags = {a for a in sys.argv[1:] if a.startswith("-")}

if "--no-build" not in flags:
    r = subprocess.run(["cargo", "build", "--quiet", "-p", "tuffcord-server", "--bin", "TUFFcord-server",
                        "-p", "tuffcord", "--bin", "tuffcord-bot"], cwd=ROOT)
    if r.returncode:
        sys.exit("build failed")

env = dict(os.environ)
if "--compat" in flags:
    r = subprocess.run([os.path.join(HERE, "build_old.sh")], cwd=ROOT, capture_output=True, text=True)
    if r.returncode:
        sys.exit("building the previous release failed:\n" + r.stderr[-2000:])
    env["OLD_BIN"] = r.stdout.strip().splitlines()[-1]
    print(f"old version: {os.path.basename(env['OLD_BIN'])}")

suites = sorted(glob.glob(os.path.join(HERE, "test_*.py")))
if args:
    suites = [s for s in suites if os.path.basename(s)[5:-3] in args]
else:  # perf needs release builds and a display; only when asked for
    suites = [s for s in suites if not s.endswith("test_perf.py")]
if any(s.endswith("test_perf.py") for s in suites) and "--no-build" not in flags:
    r = subprocess.run(["cargo", "build", "--quiet", "--release", "-p", "tuffcord-server", "--bin", "TUFFcord-server",
                        "-p", "tuffcord", "--bin", "tuffcord-bot", "--bin", "TUFFcord"], cwd=ROOT)
    if r.returncode:
        sys.exit("release build failed")
failed = []
for path in suites:
    name = os.path.basename(path)[5:-3]
    start = time.time()
    r = subprocess.run([sys.executable, path, *(["-v"] if "-v" in flags else [])], cwd=HERE, env=env,
                       capture_output=True, text=True, timeout=600)
    lines = r.stdout.strip().splitlines() or ["(no output)"]
    print(f"{name:<11} {lines[-1]}  ({time.time() - start:.0f}s)", flush=True)
    if r.returncode or "-v" in flags:
        for l in lines[:-1]:
            print(l)
        if r.returncode and r.stderr.strip():
            print("  " + "\n  ".join(r.stderr.strip().splitlines()[-8:]))
    if r.returncode:
        failed.append(name)
print("ALL PASSED" if not failed else f"FAILED: {', '.join(failed)}")
sys.exit(1 if failed else 0)
