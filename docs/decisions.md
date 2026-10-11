# Decisions

Settled choices and why. Add one when a choice would otherwise be revisited.

- **Rust desktop app (egui, glow), not a browser app.** Started as Node + a
  browser page; replaced for lower memory and global push-to-talk. egui's
  immediate mode keeps the UI code small.
- **All traffic through the server, one port.** Voice is relayed over the
  WebSocket (Opus frames), so no router or NAT setup is needed and a Cloudflare
  tunnel or reverse proxy carries everything. Files go over plain HTTP on the
  same port (since 0.7) so big transfers never delay voice.
- **Accounts with a group password to sign up (0.6).** Group password only
  creates accounts; sign-in keys are stored hashed; argon2 for passwords, run off
  the main thread so voice isn't held up.
- **Self-updating app and server.** The app swaps its own exe and restarts with
  `BACKROOM_RESUME`; the Windows server runs under a watcher process that restarts
  it (exit code 75). Releases are built by GitHub Actions from Linux
  (cross-compiled), verified with SHA256SUMS.
- **Windows' own decoders for video and audio (Media Foundation),** GPU decoding
  when available. No codecs bundled; keeps the exe small. Cost: hard to test off
  Windows; crashes in drivers are possible (handled since 0.8.1).
- **Twemoji, packed as WebP in the exe,** painted over invisible spacer fonts so
  emoji sit inline in egui text.
- **Native TLS with the system's certificate store** (SChannel on Windows) for
  both the chat connection and HTTP, so whatever Windows trusts works.
- **Rename to TUFFcord (0.8) keeps old names where older copies look for them:**
  `--version` wording, `backroom*.exe` release copies, env var names. Since
  0.8.4 crates, programs and logs say TUFFcord; a copy started as
  `backroom*.exe` renames itself (not when an old watcher started it, since
  that watcher restarts the old path), and the server's `backroom.log` moves.
- **Release builds panic = abort**, so crashes are recorded by a panic hook and a
  Windows exception filter rather than unwinding.
- **Hosting (0.10):** small Linux VPS (owner picked Hetzner Ashburn, Ubuntu
  24.04, own domain via Cloudflare Registrar, DNS only), systemd (not Docker)
  and Caddy, chosen for low memory; one server at a time (no multi-server app
  support). The watcher stays on Linux too (it restarts after self-updates);
  systemd restarts the watcher. Linux binary built on a pinned ubuntu-24.04
  runner so its glibc matches. `deploy/install.sh` + `docs/hosting.md`.
- **Full history, kept on disk (0.10, owner's choice over a bigger limit).**
  `history_limit` is now only the recent window held in memory and sent on
  sign-in; older messages go to append-only `data/history/<channel>.jsonl`,
  with only line offsets in memory (8 bytes a message) and pages of 50 read on
  `loadOlder`. Deleting an archived message overwrites its line in place with
  a same-length tombstone so offsets stay valid. Files stay until
  `max_storage_mb` (oldest first, archived included); admins are warned at
  80%/95% of it or of the disk instead of files quietly expiring. Not chosen:
  SQLite (new dependency) or object storage (S3/B2: dependency, second bill).
- **Client UI split by area, not one big `main.rs` (2026-10-10).** `main.rs`
  had grown to 3.3k lines and every feature read large slices of it. Now each
  area (server messages, sidebar, chat, dialogs, settings, ...) is its own
  file with an `impl App` block, and settings has one function per section.
  New features go in the matching module; rules in CLAUDE.md. Server
  `main.rs` (1.5k) stays whole until it grows; next cuts there are voice,
  history and attachments.
- **No rule limiting Wine checks (2026-10-10).** Audit found nothing runs
  Wine per change: tests and workflows don't, `setup.sh` only creates the
  Wine folder once, `release-check.sh` runs `--version` once per release.
  `tools/wine-ui.sh` screenshots (~1–1.5k tokens each, ~3–8k per UI check)
  are opt-in and threads already use them only for UI/Windows-only changes.
- **Adding channels: admins only, saved in the settings file (0.9.1).** Admins
  already exist and gate deleting messages; the lists are written back into
  `TUFFcord-server.toml` (other lines and comments kept) rather than a new data
  file, so the file stays the one place channels are set. Rename, delete and
  reorder aren't in the app yet (edit the file); `TEXT_CHANNELS` /
  `VOICE_CHANNELS` env vars still win and make added channels temporary.
- **Animations repaint only while moving.** The member list slides in 150 ms
  with `animate_bool_with_time_and_easing`, which asks for frames only until
  it reaches its end; the app stays idle otherwise.
- **Security pass (0.10.1, report in project files `analysis/security-review.md`).**
  Owner rule instead of a new role list: the first admin is the owner, other
  admins can't act on admins, only the owner (or the server window) gives or
  takes admin. Old apps from before accounts no longer get read-only "guest"
  access with the group password. Password hashes run two at a time
  (`limits.rs`); wrong passwords count before the hash, per address and per
  name (the account's last address is exempt so strangers can't lock the owner
  out). Per-person daily upload cap (3 GB default). VPS Caddyfile drops
  `CF-Connecting-IP` (only the Cloudflare tunnel may set it). Not done: E2E
  encryption, 2FA for all, fail2ban (cost over benefit for one friend group).
