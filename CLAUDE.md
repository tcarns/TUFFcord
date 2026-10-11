# TUFFcord

Voice and text chat for one friend group (all on Windows), in Rust. Called
Backroom before 0.8; only env vars and the compatibility names below still say
`backroom` (crates: `tuffcord`, `tuffcord-server`; bins `TUFFcord`,
`TUFFcord-server`, `tuffcord-bot`).
Repo: github.com/tcarns/TUFFcord (was tcarns/backroom; GitHub redirects the old
name, and `proto/src/update.rs` keeps it as a fallback). Owner: Tyler Carns.

## Core goal (check every request against it)
A lightweight, very memory-efficient VoIP app with modern capabilities (file
sharing, later screen sharing/streaming). Discord is the baseline to beat on
resource use; its bloat and unwanted features are what to avoid. Before any
development action, judge Tyler's request as a critical but fair colleague, not
agreeing for its own sake. If it adds memory/CPU/binary-size cost, new
dependencies, background work or Discord-style feature creep: say so plainly,
estimate the cost, propose a leaner alternative, and wait for Tyler's answer
before building. If it fits, say so in one line and proceed. Same scrutiny for
your own suggestions. Development itself stays usage-efficient.

**Direction:** the server runs on a small rented Linux
host (systemd, Caddy for HTTPS, no Docker; `docs/hosting.md`), one server at a
time; hosting at home must keep working.

## Start here
- Run `tools/setup.sh` only before the first build or test in a session (not
  for questions, docs or analysis): it installs what's missing, builds, runs the
  tests and the health check (~5 min in a bare workspace, less when the cloud
  environment has already installed the tools).
- `tools/doctor.sh` runs at session start (hook in `.claude/settings.json`); its
  `doctor:` lines show version, release, GitHub runs, disk, problems. If they're
  not in context, run it.
- `docs/status.md`: open items and next steps. **Rewrite it at the end of each
  working session** (current state, not a log).
- `docs/decisions.md`: settled choices and why; check before re-deciding.
- `docs/troubleshooting.md`: symptom → cause → fix; check before debugging,
  add an entry when something took more than one try.

## Layout
- `proto/` shared by both: `lib.rs` wire protocol (serde, camelCase JSON),
  `files.rs` file-transfer types and sniffing, `update.rs` GitHub release
  check, verified download, exe swap.
- `server/src/` tokio current_thread. `main.rs` state, message handling, chat,
  voice relay, console commands; `history.rs` recent messages, archive of older
  ones (`data/history/*.jsonl`), `loadOlder`, deleting messages; `storage.rs`
  file storage limit, admins' storage warning; `auth.rs` sign-in; `admin.rs` admin
  actions (owner rule); `limits.rs` wrong-password counts, hash and connection caps; `invites.rs` one-time invites; `sessions.rs` signed-in devices, new-address notices; `adminlog.rs` admin log (`admin_log!`); `channels.rs` admins adding, renaming, deleting, reordering channels; `accounts.rs` storage (argon2); `files.rs` HTTP endpoints on the same
  port (POST/PUT/GET /files, Range, chunked bodies); `selfupdate.rs` watcher +
  updater; `config.rs` settings file and its upgrades; `log.rs`.
- `client/src/` egui 0.32 (glow). The `App` struct is split across files,
  one `impl App` block per area:
  - `main.rs` (~800 lines): `main`, state types, `App` struct and `new`, voice
    and audio control, `handle_keys`, `eframe::App::update`.
  - `server_msgs.rs` network events and every `ServerMsg`; `sidebar.rs` main
    layout, channel lists, voice panel, own row; `chat.rs` header, message
    list, composer; `emoji_picker.rs`; `dialogs.rs` delete confirm, image
    viewer, drop overlay, volume popup, banner; `update_ui.rs` update bar and
    restart; `settings_window.rs` one function per section; `widgets.rs`
    shared drawing helpers (avatars, buttons, colors, time labels).
  - `accounts.rs` sign-in screen and account UI; `attach.rs` + `attach_ui.rs`
    attachments and players; `downloads.rs` cache downloads, preloading; `posters.rs` video
    preview frames (sender's, else taken locally); `autoscroll.rs` middle-click scroll;
    `volume_ui.rs` speaker button and volume popup; `channels_ui.rs` channel admin (+, right-click menu, drag to reorder);
    `members.rs`; `security_ui.rs` devices, invites, admin log in Settings > Account; `chat_text.rs`; `theme.rs`; `history_ui.rs` loading older
    messages on scroll up, admins' storage bar.
  - Library (`lib.rs`, shared with `bin/bot.rs`, no UI): `media.rs` Media
    Foundation player (Windows only); `crash.rs`; `net.rs`, `voice.rs`,
    `audio.rs` (cpal, opus); `files.rs` HTTP transfers; `settings.rs`,
    `applog.rs`, `updater.rs`, `emoji.rs`, `twemoji.rs`, `keys.rs`;
    `bin/bot.rs` headless test client (tones, `--send-file`).
- `deploy/install.sh` sets up the server on a rented Ubuntu machine (systemd,
  Caddy); guide in `docs/hosting.md`.
- `tests/` end-to-end suites (Python), `tools/` scripts, `.github/workflows/`
  `tests.yml` (every push) and `release.yml` (publishes when the version rises).

## Where new code goes (keep `main.rs` small)
- New features go in the module for their area (list above), or a new module
  when no area fits; `main.rs` gets only new `App` fields, their setup in
  `new`, and one-line calls from `update`. Same for server `main.rs`: new
  areas get their own file (voice, history, attachments are the next cuts).
- A new screen, dialog or settings section is its own function; a settings
  section is a `fn x_section(&mut self, ui)` in `settings_window.rs`. When a
  change takes a function past ~200 lines or a file past ~1,000, split it in
  that change (`tools/doctor.sh` lists files over the limit).
- Shared drawing helpers go in `widgets.rs`; logic with no UI goes in the
  library (`lib.rs` side) so the bot and unit tests can use it.
- New modules: `//!` line saying what's in it, explicit `use crate::...`
  imports (`use super::*` only in test modules), `pub(crate)` only for what other files call.
  Add the file to the Layout list above in the same commit.

## Never change (older copies depend on them)
- Env vars `BACKROOM_SERVER_CHILD`, `BACKROOM_RESUME`; watcher exit code 75.
- `--version` prints exactly `backroom-server <version>` (0.7.1 and earlier
  refuse an update otherwise). Readers accept `TUFFcord-server ` too.
- Releases carry `backroom.exe` and `backroom-server.exe` copies alongside
  `TUFFcord*.exe`, all listed in `SHA256SUMS.txt`; `TUFFcord-server-linux-x86_64`
  is what Linux servers update from (built on Ubuntu 24.04).
- Protocol: only add optional fields/messages; old clients skip unknown types.
- `backroom-server.toml` / `TUFFcord-server.toml` hold the group password:
  never commit (both in .gitignore).

## Commands
- Unit tests: `cargo test --workspace`
- End to end: `tests/run.py` (one line per suite; logs in `target/tests/`),
  `tests/run.py files delete`, `--compat` adds old/new version pairs (previous
  release built once into `target/compat/`), `-v` shows every check.
- Windows: `tools/windows.sh check` (fast) / `build`.
- Wine UI: `tools/wine-ui.sh start <exe>`, `shot out.png 400x80+340+440`, `stop`.
- Release: bump `version` in root `Cargo.toml`, add a `## x.y.z` section at the
  top of `CHANGELOG.md` (it becomes the notes the app shows), commit, push to
  main, approve the run (signing setup: `docs/releasing.md`,
  `tools/release-key.py`). Then `tools/release-check.sh` (run passed, files, checksums, old-name
  copies, zip, `--version`). `gh release` uses GraphQL, which the proxy blocks;
  use `gh api` (REST).
- Commits: author Tyler Carns (repo-local config is set); end messages with
  the Co-Authored-By / Claude-Session lines from the session's instructions.

## Workspace quirks
- Disk allowance is small. `tools/doctor.sh` prunes build leftovers when space
  runs low. "Bus error" from the linker means the disk is full.
- `pkill -f <pattern>` matches its own shell and kills it (exit 144). Use
  `pkill -x <name>` or filter `ps -eo pid,args`.
- The proxy blocks repo settings changes and GraphQL; plain `gh api` REST works.
  Other websites only via WebFetch.
- Wine: real GPU behaviour can't be tested; software video decoding gives no
  picture. Media Foundation frames work on the GPU path for videos without sound.

## Working habits (usage)
- Read the parts of files you need (grep for the function first); `main.rs` is big.
- Filter command output (`tail`, `grep -E "^(error|warning)"`); never paste
  whole build logs.
- Prefer the test runner over ad-hoc scripts; add a suite or a check to
  `tests/` instead of writing throwaway tests.
- Screenshots: crop to the part being checked; full window only for layout.
- Try anything that may be blocked (permissions, network) before building on it.
- Keep wrap-ups short: what changed, how it was checked, what the user does.
- Keep this file under ~120 lines; detail goes in `docs/`.
