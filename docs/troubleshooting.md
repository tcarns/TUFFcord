# Troubleshooting

Problems already solved once: symptom → cause → fix. Add an entry whenever a
bug or environment problem takes more than one try to understand.

## People using it
- **App freezes ("not responding") seconds into a video, Windows volume mixer
  hangs too; logged as "closed unexpectedly while playing", with or without
  the graphics card** → lock-order deadlock in `client/src/media.rs`: the player
  thread held `shared` while calling `engine.GetCurrentTime()`, while Media
  Foundation held its engine/audio-session lock and called `EventNotify`, which
  waits for `shared`. Fixed in 0.8.3: never call the engine while holding
  `shared`; `EventNotify` ignores events other than metadata/ended/error.
- **"native-tls: unable to find any user-specified roots in the final cert
  chain" when sending files** → HTTP transfers used ureq's built-in root list,
  which lacked some Cloudflare chains. Fixed in 0.7.1: `RootCerts::PlatformVerifier`.
- **Uploads refused through Cloudflare ("Missing or too long request", 411)** →
  cloudflared re-sends bodies with `Transfer-Encoding: chunked`. Server parses
  chunked bodies since 0.7.1.
- **Images from before 0.7 show "Couldn't show this image"** → old servers kept
  images only for the WebSocket path; the app falls back to asking over the chat
  connection.
- **Friends must get a new address after the host restarts cloudflared** →
  "quick tunnel" addresses (`*.trycloudflare.com`) change each run. Fix: a named
  tunnel, or the planned rented server with a domain.
- **Signed out after the app updated itself** (0.7.1 and earlier) → the resume
  hand-over had no sign-in key when the key itself had been used to sign in.
  Fixed on the receiving side in 0.8.0.
- **App closes while playing a video, nothing in the log** (before 0.8.1) →
  panic = abort with no hook, or a crash inside Windows/driver code. Since 0.8.1:
  see `last-crash.txt` / the app log / the server log line "App error on …".
  Windows also records it: Reliability Monitor → the red X → Technical details
  (faulting module name, exception code).
- **Server won't start: "Port … is already in use"** → another copy is running
  (Task Manager shows two TUFFcord-server.exe for one copy: watcher + server).

## Development workspace
- **Linker "Bus error" / "No space left on device"** → the disk allowance is
  full. Run `tools/doctor.sh` (prunes builds), or delete `target/debug/incremental`
  and old `target/compat/*`.
- **Shell command exits 144 after `pkill -f …`** → the pattern matched the shell
  running it. Use `pkill -x name` or filter `ps -eo pid,args`.
- **"certificate verify failed" in tunnel tests** → the test CA isn't trusted;
  `tests/test_tunnel.py` sets `SSL_CERT_FILE` to a bundle with it.
- **`gh release …` fails with 403 (GraphQL)** → the proxy only allows REST:
  `gh api repos/tcarns/TUFFcord/releases/tags/vX`.
- **`gh api -X PATCH repos/…` refused ("Repository settings writes are not
  permitted")** → owner must change repo settings on github.com.
- **Windows build: "cannot find rsbegin.o / rsend.o"** → Ubuntu's rustc ships no
  Windows startup objects; `tools/setup.sh` now builds them from
  `library/rtstartup` into its rustlib folder (first seen in the cloud environment).
- **setup.sh: "definition of an unknown lang item: `drop_in_place`"** → a
  rustup rustc on PATH compiled the 1.91 startup objects; setup.sh now calls
  `/usr/lib/rust-1.91/bin/rustc` for them.
- **`tools/wine-ui.sh start` never returns** (seen once in the cloud
  environment, window was up) → run it in the background, or start
  `wine TUFFcord.exe` directly with `DISPLAY=:97` and take shots with `shot`.
- **Wine: the app uses ~50% CPU while idle** (UI thread, plus wineserver) →
  Wine, not the app: 0.9.0's release exe does the same and the number doesn't
  change with what's on screen. Idle CPU can't be judged under Wine; compare
  against the previous release's exe instead (asset download via
  `gh api -H "Accept: application/octet-stream" .../releases/assets/<id>`).
- **Wine: blank screenshots** → taken before the window drew; use
  `tools/wine-ui.sh start`, which waits for it. "Can't open X server" → the
  virtual display died; `start` brings it back.
- **Wine: video shows no picture ("Couldn't show this video here")** → software
  decoding under Wine gives no frames, and videos with sound give none even on
  the GPU path. Not a bug on real Windows.
- **Test server "Port already in use" / stale results** → leftover processes
  from an earlier run; `tools/doctor.sh` ends them. The test suites pick free
  ports and clean up after themselves.
- **Release step 403 from api.github.com for `tcarns/TUFFcord`** inside this
  workspace → the proxy only allows repos attached to the session; on the real
  internet a missing repo is a 404 and the app falls back to the old name.
- **`release-check.sh`: "checksums are signed ... FAIL" on a good release** →
  `ssh-keygen` missing in the workspace (verify-release.sh needs it) → install
  `openssh-client` (`tools/setup.sh` does).
