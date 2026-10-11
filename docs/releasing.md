# Releasing

Pushing a higher `version` in `Cargo.toml` to main starts the Release
workflow (`.github/workflows/release.yml`). It tests and builds everything and
puts it in a **draft** release. Nothing reaches anyone until the owner signs
the draft on his own PC and runs the **Publish release** workflow with the
signature (`.github/workflows/publish.yml`).

## Why

Apps and servers from 0.11.1 offer and install an update only if its
`SHA256SUMS.txt` is signed with the owner's key (`ssh-keygen -Y sign`, public
half in `RELEASE_KEY` in `proto/src/update.rs`) and the file's first line
names that version. The private key and its passphrase stay on the owner's
PC, so neither a stolen GitHub login nor a compromised build dependency can
sign a release, and an old release can't be passed off as a new one. Servers
still wait a day before installing a release by themselves (typing `update`
installs at once), so a bad release can be pulled first.

## One-time setup (owner, done 2026-10-11)

In **PowerShell**:

```
ssh-keygen -t ed25519 -C tuffcord-sign -f $HOME\tuffcord-sign
```

with a passphrase. The `ssh-ed25519 …` line from `tuffcord-sign.pub` is
`RELEASE_KEY`. Back up `tuffcord-sign` somewhere offline (a password manager
or a USB stick); without it, the next update has to be installed by hand on
every PC and server.

A hardware key works too: `ssh-keygen -t ed25519-sk` (needs OpenSSH 8.2+; a
YubiKey or similar), and every signature then needs a tap on the key.

## Each release

1. Bump the version, add the `CHANGELOG.md` section, push to main. The
   Release workflow runs (~15 minutes) and leaves a draft.
2. On GitHub, **Releases** → the draft → download **SHA256SUMS.txt** (to
   Downloads; delete older copies there first so the name stays exact).
3. In PowerShell, sign it and copy the signature:

   ```
   ssh-keygen -Y sign -f $HOME\tuffcord-sign -n tuffcord-release $HOME\Downloads\SHA256SUMS.txt
   Get-Content $HOME\Downloads\SHA256SUMS.txt.sig | Set-Clipboard
   ```

   It asks for your passphrase. If it says `unknown option -- Y`, Windows'
   OpenSSH is too old; use Git's instead:
   `& "C:\Program Files\Git\usr\bin\ssh-keygen.exe" -Y sign ...` (same rest).
4. GitHub → **Actions** → **Publish release** → **Run workflow**: type the
   version (e.g. `0.11.1`), paste into *signature* (it's fine that the line
   breaks disappear), **Run workflow**. It checks the signature and the
   version line, attaches the signature and publishes. A wrong file or
   passphrase-less key just fails there; nothing is published.
5. `tools/release-check.sh` checks files, checksums, the signature and
   `--version`.

To throw a draft away (e.g. to rebuild it), delete it on the Releases page
and push again or run the Release workflow by hand.

## Changeover (0.11.1 only)

Copies up to 0.11.0 only accept the old raw signature made by the release
workflow with `RELEASE_SIGNING_KEY` (GitHub environment `release`). So for
0.11.1 the workflow's `old-sign` job still makes that signature (it waits for
the Approve click as before, and sees nothing but `SHA256SUMS.txt`). After
0.11.1 is out: delete the `old-sign` job and the `RELEASE_SIGNING_KEY`
secret. Anyone still on 0.11.0 or older after that downloads the new version
from the releases page once.

## A new key (lost or leaked)

Make a new key as above, put its public half in `RELEASE_KEY`, release, and
sign with the new key. Copies carrying the old key refuse that release:
people download it once from the releases page (servers: replace the program
by hand). From then on updates work as before.
