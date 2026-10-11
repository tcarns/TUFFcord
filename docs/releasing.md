# Releasing

Pushing a higher `version` in `Cargo.toml` to main starts the Release
workflow (`.github/workflows/release.yml`). From 0.11 it runs in the GitHub
environment **release**, which holds the signing key and waits for the owner
to approve each release.

## Why

Apps and servers from 0.11 install an update only if `SHA256SUMS.txt` is
signed with the release key (Ed25519; public half in `proto/src/update.rs`,
`RELEASE_KEY`). A stolen GitHub token can't read the key, and a push to main
can't publish without the owner's Approve click. Servers also wait a day
before installing a release by themselves (typing `update` installs at once),
so a bad release can be pulled first.

## One-time setup (owner)

Make the key on your own PC, in **PowerShell** (OpenSSH ships with Windows):

```
ssh-keygen -t ed25519 -C tuffcord-release -f $HOME\tuffcord-release
type $HOME\tuffcord-release.pub
```

Press Enter twice at the passphrase prompts (the workflow can't type one;
the GitHub secret and the approval step are what protect it). The second line
prints the public half, `ssh-ed25519 AAAA… tuffcord-release`; its last 32
bytes in hex go into `RELEASE_KEY` in `proto/src/update.rs`
(`python3 tools/release-key.py < tuffcord-release` prints that hex too). The
private key never leaves your PC except into GitHub's secret below. Then in
GitHub's website:

1. Repository → **Settings** → **Environments** → **New environment**, name
   it `release`.
2. Tick **Required reviewers**, add yourself, **Save protection rules**.
3. Under **Deployment branches and tags** choose **Selected branches and
   tags** and add `main`.
4. **Add environment secret**: name `RELEASE_SIGNING_KEY`, value: the whole
   private key file `tuffcord-release` (`-----BEGIN OPENSSH PRIVATE KEY-----`
   to `-----END OPENSSH PRIVATE KEY-----`; a PEM key works too).
5. Keep `tuffcord-release` somewhere safe and offline (a password manager)
   and delete other copies. Without it, a lost secret means a new key, and every app and
   server would have to be updated by hand once.

## Each release

1. Bump the version, add the `CHANGELOG.md` section, push to main.
2. GitHub emails "Review pending deployments" (or see the Actions tab). Open
   the Release run → **Review deployments** → tick `release` → **Approve and
   deploy**. Works from the GitHub phone app too.
3. `tools/release-check.sh` checks the files, checksums and the signature.

If the secret is missing the run stops at "Sign the checksums" and nothing is
published.

## A new key (lost or leaked)

1. Make a new key as in the one-time setup.
2. Put its public half in `RELEASE_KEY` in `proto/src/update.rs`, replace
   the environment secret, release. Copies that carry the old key refuse this
   release: people download it once from the releases page (servers: replace
   the program by hand).
