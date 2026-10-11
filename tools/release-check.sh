#!/usr/bin/env bash
# Check a published release: the Release run passed, it's the latest, all files
# are there, checksums match, the old-name copies are identical, the zip holds
# the right files, and the server reports the right version (via Wine).
#   tools/release-check.sh          the version in Cargo.toml
#   tools/release-check.sh v0.8.1
# Prints one line per check; exits non-zero if any failed.
set -uo pipefail
cd "$(dirname "$0")/.."
REPO=$(git remote get-url origin 2>/dev/null | sed -E 's#^.*github.com[:/]##; s#\.git$##'); REPO=${REPO:-tcarns/TUFFcord}
tag="${1:-v$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)}"
v="${tag#v}"
fails=0
ok() { if [ "$1" = 0 ]; then echo "  ok   $2"; else echo "  FAIL $2"; fails=$((fails + 1)); fi; }

run=$(gh api "repos/$REPO/actions/workflows/release.yml/runs?per_page=1" --jq '.workflow_runs[0].conclusion // .workflow_runs[0].status' 2>/dev/null)
[ "$run" = success ]; ok $? "latest Release run: $run"
[ "$(gh api "repos/$REPO/releases/latest" --jq .tag_name 2>/dev/null)" = "$tag" ]; ok $? "$tag is the latest release"

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
urls=$(gh api "repos/$REPO/releases/tags/$tag" --jq '.assets[].browser_download_url' 2>/dev/null)
want="TUFFcord.exe TUFFcord-server.exe backroom.exe backroom-server.exe TUFFcord-server-linux-x86_64 SHA256SUMS.txt TUFFcord-$tag-windows.zip"
for f in $want; do
  grep -q "/$f\$" <<<"$urls"; ok $? "has $f"
done
for u in $urls; do curl -sSL -o "$dir/$(basename "$u")" "$u"; done
(cd "$dir" && sha256sum --quiet -c SHA256SUMS.txt >/dev/null 2>&1); ok $? "checksums match"
cmp -s "$dir/TUFFcord.exe" "$dir/backroom.exe" && cmp -s "$dir/TUFFcord-server.exe" "$dir/backroom-server.exe"
ok $? "old-name copies are identical (for copies from before the rename)"
# From 0.11.1 the owner signs the checksums on his PC (ssh-keygen); 0.11.0
# and the 0.11.1 changeover also carry the old raw signature 0.11.0 checks.
if [ "$(printf '%s\n' "$v" 0.11.1 | sort -V | head -n1)" = 0.11.1 ]; then
  [ -f "$dir/SHA256SUMS.txt.sshsig" ]; ok $? "has SHA256SUMS.txt.sshsig"
fi
if [ -f "$dir/SHA256SUMS.txt.sshsig" ]; then
  tools/verify-release.sh "$dir/SHA256SUMS.txt" "$dir/SHA256SUMS.txt.sshsig" "$v" >/dev/null 2>&1
  ok $? "checksums are signed with the owner's key and name $v"
fi
if [ -f "$dir/SHA256SUMS.txt.sig" ]; then
  pub=565816b56df7a234d2f6ff1eeaeee1fba5ab334d8f69dd813938b7b01329eebe  # the key 0.11.0 carries
  printf '302a300506032b6570032100%s' "$pub" | sed 's/../\\x&/g' | xargs -0 printf '%b' > "$dir/pub.der"
  tr -d '\n' < "$dir/SHA256SUMS.txt.sig" | sed 's/../\\x&/g' | xargs -0 printf '%b' > "$dir/sig.bin"
  openssl pkeyutl -verify -pubin -keyform DER -inkey "$dir/pub.der" -rawin -in "$dir/SHA256SUMS.txt" -sigfile "$dir/sig.bin" >/dev/null 2>&1
  ok $? "old-format signature (for 0.11.0 copies) checks out"
fi
list=$(unzip -Z1 "$dir/TUFFcord-$tag-windows.zip" 2>/dev/null)
grep -qx TUFFcord/TUFFcord.exe <<<"$list" && grep -qx TUFFcord/TUFFcord-server.exe <<<"$list" && grep -qx TUFFcord/README.txt <<<"$list"
ok $? "zip has TUFFcord/ with both programs and README.txt"
if command -v wine >/dev/null; then
  said=$(WINEDEBUG=-all timeout 60 wine "$dir/TUFFcord-server.exe" --version 2>/dev/null | tr -d '\r')
  [ "$said" = "backroom-server $v" ]; ok $? "server --version says \"backroom-server $v\" (older servers need exactly that)"
else
  echo "  --   no Wine: skipped the --version check"
fi
chmod +x "$dir/TUFFcord-server-linux-x86_64" 2>/dev/null
said=$(timeout 30 "$dir/TUFFcord-server-linux-x86_64" --version 2>/dev/null)
[ "$said" = "backroom-server $v" ]; ok $? "Linux server runs and says \"backroom-server $v\""
[ $fails = 0 ] && echo "release $tag: ALL OK" || { echo "release $tag: $fails FAILED"; exit 1; }
