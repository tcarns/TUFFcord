# Status

Where things stand. Rewrite (don't append) at the end of each working session.

**Last updated:** 2026-10-11. **Released:** 0.11.0; 0.11.1 built as a draft, waiting for the owner's signature.

## Waiting on the owner
- Rented server deferred (2026-10-11): development continues against his home
  Windows server. When he's ready he follows `docs/hosting.md`; `install.sh`
  hasn't run on a real VPS yet.
- From 0.11.1 he signs every release on his PC (`ssh-keygen -Y sign`,
  passphrase) and runs the Publish release workflow (`docs/releasing.md`).
  Key `tuffcord-sign` is on his PC only; public half is `RELEASE_KEY` in
  `proto/src/update.rs`. 0.11.1 also needs his Approve click once more
  (changeover signature for 0.11.0 copies).
- After 0.11.1 is published: delete the `old-sign` job from `release.yml`
  (and make `draft` stop needing it) and ask him to delete the
  `RELEASE_SIGNING_KEY` secret.

## Security pass (0.10.1 and 0.11.0)
Report with every finding: `analysis/security-review.md` in the project files.
- 0.10.1: hash and connection caps, wrong-password limits per address and per
  name (`server/src/limits.rs`), owner rule for admin actions (now
  `server/src/admin.rs`), no group-password-only access, 8-character
  passwords, daily upload cap, image size limits, more program extensions ask
  before opening, bidi characters stripped from names, VPS hardening in
  `deploy/install.sh`, pinned workflow actions.
- 0.11.0: signed updates (Ed25519 over `SHA256SUMS.txt`, `.sig` file; apps
  and servers refuse unsigned releases; servers auto-install only after a
  release is a day old), saved sign-in sealed with DPAPI (`settings.rs`
  `mod sealed`), one-time invites, signed-in devices with sign-out,
  new-address notices, admin log, look-alike names refused, plain-text warning
  on the sign-in screen. Checked: `tests/test_security.py` (19), selfupdate
  suite (other-key release refused), all suites, Wine screenshots of the
  Account page; DPAPI round trip worked under Wine.
- Second review (`analysis/security-review-2.md`): #1-#4 and #3 done in 0.11.1
  (local signing, signature checked when an update is found, version line in
  `SHA256SUMS.txt`, deleted text channels go to `data/trash` for 30 days,
  `trash` / `restore <n>` in the server window). Open, low: #5-#11 (old
  plain-text sign-ins, IPv6 /64 limits, 17 MB pre-sign-in messages,
  token new-address notice, admin log size, `-ms` extensions, `con`/`nul`
  channel names).
- Still open from the first report ("later"): nothing scheduled. Left out on
  purpose: 2FA, E2E encryption, fail2ban (see `docs/decisions.md`).
- 0.10.x copies have no key check and install 0.11.0 normally; from 0.11.0 on
  every update must be signed.

## Check early next session
- `tests/run.py --compat` in the Release workflow: confirm it ran in 0.11.0.
- Small fix pending: when `gh api` is refused, `tools/doctor.sh` prints GitHub's
  JSON error into its status line; send gh's errors to /dev/null and print `?`.

## Next for hosting (after the owner's server is up)
- Watch the first self-update on the VPS (journal should show the restart,
  a day after the release unless `update` is typed).
- Optional, later: HTTPS built into the server (drops Caddy, ~30 MB).

## Recent releases (all confirmed by the owner unless noted)
- 0.10.0: full history on disk with load-older, admin storage bar, Linux
  server with self-update.
- 0.9.3: video preview frames, middle-click auto scroll, button hover.
- 0.9.2: resizable sidebar and member list; admin rename, delete, reorder.
- 0.9.1: admins add channels; member list slide.
- 0.9.0: window size remembered, media preload and cache settings.
- Gaps known: window may open off screen after a monitor is unplugged; big
  clips that aren't preloaded show black until played.

## Workspace
- The "TUFFcord" cloud environment installs the toolchain before a thread
  starts (`analysis/cloud-env-setup.sh` in project files). Windows builds and
  Wine work there. `tools/setup.sh` only before building or testing.
- Files over 1,000 lines (doctor lists them): `client/attach_ui.rs`,
  `server/files.rs`, `server/main.rs`; split the one you add to.

## Undecided options (from the efficiency review)
Full review: `analysis/token-efficiency-review.md` in the project files.
- C: support only current + previous version; drop pre-0.7 compatibility code.
- D (server half): split server `main.rs`; cuts: voice, history, attachments.
- E: automated UI tests (egui renders without a window) instead of screenshots.
- H: keep the built-in video player, or open videos in the system player.
