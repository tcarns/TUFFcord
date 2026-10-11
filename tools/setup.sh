#!/usr/bin/env bash
# Set up a fresh workspace (Ubuntu 24.04) for TUFFcord: everything needed to
# build, test, cross-compile for Windows and look at the app under Wine.
# Safe to run again: it skips what's already there. Takes a few minutes the
# first time (mostly apt and the first Wine start).
#
# Why the Windows toolchain looks unusual: rustup's downloads are blocked in
# these workspaces, so Windows builds use Ubuntu's rustc-1.91 and build Rust's
# standard library from source (-Z build-std). Ubuntu's copy of that source
# drops a Windows dependency; this script copies it and puts it back.
set -euo pipefail
cd "$(dirname "$0")/.."
STD_SRC="${STD_SRC:-$HOME/.cache/tuffcord/rust-std-src/library}"
step() { printf '\nsetup: %s\n' "$*"; }
SUDO=""; [ "$(id -u)" = 0 ] || SUDO="sudo"

pkgs=(build-essential pkg-config cmake libasound2-dev openssl unzip curl
      gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 rustc-1.91 cargo-1.91 rust-1.91-src
      wine wine64 xvfb xdotool imagemagick python3-pip
      libxkbcommon-x11-0 libgl1-mesa-dri libegl1)
missing=()
for p in "${pkgs[@]}"; do dpkg -s "$p" >/dev/null 2>&1 || missing+=("$p"); done
if [ ${#missing[@]} -gt 0 ]; then
  step "installing ${missing[*]}"
  $SUDO apt-get update -qq
  DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y -qq "${missing[@]}" >/dev/null
fi

if ! command -v cargo >/dev/null; then
  step "no cargo on PATH; using Ubuntu's 1.91 for Linux builds too"
  export PATH=/usr/lib/rust-1.91/bin:$PATH
fi

python3 -c "import websockets" 2>/dev/null || {
  step "installing python websockets"
  pip install --quiet --break-system-packages websockets 2>/dev/null || pip install --quiet websockets
}

if [ ! -f "$STD_SRC/std/Cargo.toml" ] || ! grep -q windows_targets "$STD_SRC/std/Cargo.toml"; then
  step "preparing the Rust standard library source for Windows builds"
  src=$(ls -d /usr/src/rustc-1.91*/library | head -1)
  rm -rf "$STD_SRC"; mkdir -p "$(dirname "$STD_SRC")"
  cp -r "$src" "$STD_SRC"
  python3 - "$STD_SRC/std/Cargo.toml" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
if "windows_targets" not in s:
    head, sep, rest = s.partition("[dependencies]")
    s = head + "[target.'cfg(windows)'.dependencies.windows-targets]\npath = \"../windows_targets\"\n\n" + sep + rest
    s = s.replace("windows_raw_dylib = []", 'windows_raw_dylib = ["windows-targets/windows_raw_dylib"]')
    open(p, "w").write(s)
EOF
fi

# Ubuntu's rustc has no Windows startup objects; without them linking fails
# with "cannot find rsbegin.o".
win_lib=/usr/lib/rust-1.91/lib/rustlib/x86_64-pc-windows-gnu/lib
if [ ! -f "$win_lib/rsend.o" ]; then
  step "building Rust's Windows startup objects"
  $SUDO mkdir -p "$win_lib"
  for f in rsbegin rsend; do
    # Ubuntu's 1.91 compiler, matching the source (a rustup rustc on PATH isn't).
    $SUDO env RUSTC_BOOTSTRAP=1 /usr/lib/rust-1.91/bin/rustc --target x86_64-pc-windows-gnu --crate-type=lib --emit=obj \
      -C panic=abort -C opt-level=3 -o "$win_lib/$f.o" "$STD_SRC/rtstartup/$f.rs"
  done
fi

if [ ! -d "$HOME/.wine" ]; then
  step "first Wine start (creates its Windows folder)"
  WINEDEBUG=-all timeout 300 wineboot -i >/dev/null 2>&1 || true
fi

if [ -z "$(git config user.email || true)" ]; then
  git config user.name "Tyler Carns"
  git config user.email "tyler.carns.i@gmail.com"
fi

step "building and running the tests (first build takes a few minutes)"
cargo build --quiet -p tuffcord-server --bin TUFFcord-server -p tuffcord --bin tuffcord-bot --bin TUFFcord
STD_SRC="$STD_SRC" tools/windows.sh check
tests/run.py --no-build
tools/doctor.sh
