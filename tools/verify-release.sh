#!/usr/bin/env bash
# Check a release's signature the way the apps do: SHA256SUMS.txt signed with
# the owner's key (RELEASE_KEY in proto/src/update.rs, `ssh-keygen -Y sign -n
# tuffcord-release`), and its first line naming the version.
#   tools/verify-release.sh SHA256SUMS.txt SHA256SUMS.txt.sshsig 0.11.1
# The signature may be the .sshsig file as ssh-keygen wrote it, or the same
# text pasted with its line breaks lost (as GitHub's "Run workflow" box does).
# Needs ssh-keygen (OpenSSH 8.1+). Exits non-zero if anything is off.
set -euo pipefail
sums=$1 sig=$2 version=${3#v}
root=$(cd "$(dirname "$0")/.." && pwd)
key=$(grep -A1 '^const RELEASE_KEY' "$root/proto/src/update.rs" | grep -oE '"(sk-)?ssh-ed25519[^ ]* [A-Za-z0-9+/=]+' | tr -d '"')
[ -n "$key" ] || { echo "no RELEASE_KEY in proto/src/update.rs" >&2; exit 1; }
first=$(head -n1 "$sums" | tr -d '\r')
[ "$first" = "version $version" ] || { echo "SHA256SUMS.txt says \"$first\", not \"version $version\"" >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Rebuild the armored form from whatever came in.
body=$(sed -e 's/-----BEGIN SSH SIGNATURE-----//g' -e 's/-----END SSH SIGNATURE-----//g' "$sig" | tr -d ' \r\n\t')
{ echo "-----BEGIN SSH SIGNATURE-----"; fold -w 70 <<<"$body"; echo "-----END SSH SIGNATURE-----"; } > "$tmp/sig"
echo "tuffcord $key" > "$tmp/allowed"
ssh-keygen -Y verify -f "$tmp/allowed" -I tuffcord -n tuffcord-release -s "$tmp/sig" < "$sums"
