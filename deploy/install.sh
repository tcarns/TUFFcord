#!/usr/bin/env bash
# Sets up a TUFFcord server on a fresh Ubuntu 24.04 machine (see docs/hosting.md).
# Run as root, with the address people will connect to:
#   curl -fsSL https://raw.githubusercontent.com/tcarns/TUFFcord/main/deploy/install.sh | bash -s chat.example.com
# Safe to run again: it keeps the server's settings and data, and updates the
# service and Caddy files.
set -euo pipefail
domain="${1:?usage: install.sh <domain, e.g. chat.example.com>}"
repo=tcarns/TUFFcord
dir=/opt/tuffcord
asset=TUFFcord-server-linux-x86_64
[ "$(id -u)" = 0 ] || { echo "Run this as root (sudo)."; exit 1; }

echo "== Packages: Caddy (HTTPS) and the firewall"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q curl gnupg ufw debian-keyring debian-archive-keyring apt-transport-https
# Caddy's own repository, as in https://caddyserver.com/docs/install#debian-ubuntu-raspbian
curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key \
  | gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt \
  > /etc/apt/sources.list.d/caddy-stable.list
apt-get update -q
apt-get install -y -q caddy
ufw allow OpenSSH >/dev/null
ufw allow 80/tcp >/dev/null
ufw allow 443/tcp >/dev/null
ufw --force enable >/dev/null
# SSH with keys only (the hosting guide adds yours when the server is made).
if [ -s /root/.ssh/authorized_keys ]; then
  mkdir -p /etc/ssh/sshd_config.d
  printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin prohibit-password\n' \
    > /etc/ssh/sshd_config.d/10-tuffcord.conf
  systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true
fi

echo "== TUFFcord server in $dir"
id tuffcord >/dev/null 2>&1 || useradd --system --home-dir "$dir" --shell /usr/sbin/nologin tuffcord
mkdir -p "$dir"
if [ ! -x "$dir/TUFFcord-server" ]; then
  tmp=$(mktemp -d)
  base="https://github.com/$repo/releases/latest/download"
  curl -fsSL -o "$tmp/$asset" "$base/$asset"
  curl -fsSL -o "$tmp/SHA256SUMS.txt" "$base/SHA256SUMS.txt"
  (cd "$tmp" && grep -E "[ *]$asset\$" SHA256SUMS.txt | sha256sum -c --quiet -)
  install -m 755 "$tmp/$asset" "$dir/TUFFcord-server"
  rm -rf "$tmp"
fi
"$dir/TUFFcord-server" --version
chown -R tuffcord:tuffcord "$dir"
# Settings, accounts and files are private to the server's user.
chmod 750 "$dir"
[ -f "$dir/TUFFcord-server.toml" ] && chmod 600 "$dir/TUFFcord-server.toml"
[ -d "$dir/data" ] && chmod 700 "$dir/data"

# Only Caddy talks to the server (HOST=127.0.0.1); it updates itself (the
# watcher restarts it), and systemd restarts it if it stops.
cat > /etc/systemd/system/tuffcord.service <<UNIT
[Unit]
Description=TUFFcord server
After=network-online.target
Wants=network-online.target

[Service]
User=tuffcord
WorkingDirectory=$dir
ExecStart=$dir/TUFFcord-server
Environment=HOST=127.0.0.1 NO_COLOR=1
Restart=on-failure
RestartSec=3
NoNewPrivileges=yes
# A runaway server is restarted instead of taking SSH down with it.
MemoryMax=1G
UMask=0077
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=$dir

[Install]
WantedBy=multi-user.target
UNIT

# Caddy gets and renews the HTTPS certificate for the domain by itself.
port=$(grep -E '^port *= *[0-9]+' "$dir/TUFFcord-server.toml" 2>/dev/null | grep -oE '[0-9]+' || echo 3000)
cat > /etc/caddy/Caddyfile <<CADDY
$domain {
	reverse_proxy 127.0.0.1:$port {
		# The server trusts this header from local proxies (it's how the
		# Cloudflare tunnel says who's connecting). Here nothing in front sets
		# it, so drop whatever a visitor sends; X-Forwarded-For comes from Caddy.
		header_up -CF-Connecting-IP
	}
}
CADDY

# "tuffcord invite" (and the other console commands) for the running server.
cat > /usr/local/bin/tuffcord <<'CMD'
#!/bin/sh
if [ $# -eq 0 ]; then
  echo "Usage: tuffcord <command>   (tuffcord help lists them)"
  exit 1
fi
exec sudo -u tuffcord /opt/tuffcord/TUFFcord-server "$@"
CMD
sed -i "s|/opt/tuffcord|$dir|" /usr/local/bin/tuffcord
chmod 755 /usr/local/bin/tuffcord

systemctl daemon-reload
systemctl enable --now tuffcord
systemctl restart tuffcord
systemctl reload caddy || systemctl restart caddy
sleep 2
echo
echo "== Done"
systemctl --no-pager --lines=0 status tuffcord | head -3
echo "Address for the app:  $domain"
echo "Settings:             $dir/TUFFcord-server.toml (restart after editing: systemctl restart tuffcord)"
echo "Log:                  journalctl -u tuffcord -f"
echo "Invite someone:       tuffcord invite   (other commands: tuffcord help)"
