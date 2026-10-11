# Hosting the server on a rented Linux machine

Moves the TUFFcord server off your PC onto a small rented server (a "VPS")
with its own web address and HTTPS. About 30 minutes, once. Costs: the server
about $5–6 a month, the domain about $10 a year.

What runs there: `TUFFcord-server` (tens of MB of memory) and Caddy, which
handles HTTPS and certificates (about 30–40 MB). Both start with the machine
and restart if they stop. The server updates itself like the Windows one does.

You'll need: a card for the two purchases, and PowerShell on your PC (built
into Windows; it has `ssh` and `scp`).

## 1. Buy the domain (Cloudflare Registrar)

Cloudflare sells domains at cost, with no markup and free DNS.

1. Go to https://dash.cloudflare.com and sign up (or sign in).
2. Left menu: **Domain Registration → Register Domains**. Search for a name
   (for example `tuffcord.net`), pick one, pay. A `.com` is about $10 a year.
   Turn auto-renew on, or the address stops working after a year.
3. Leave this tab open; step 3 adds one record here.

You'll use a sub-address of it for the server, for example `chat.tuffcord.net`.

## 2. Rent the server (Hetzner, Ashburn)

1. Sign up at https://www.hetzner.com/cloud (they may ask for ID once).
2. **Create a project**, then **Add Server**:
   - Location: **Ashburn, VA** (US East).
   - Image: **Ubuntu 24.04** (the server program is built for it).
   - Type: the smallest shared vCPU one (2 vCPU, 2 GB RAM, 40 GB disk).
   - SSH key: on your PC, open PowerShell and run `ssh-keygen` (press Enter
     to every question), then `type $env:USERPROFILE\.ssh\id_ed25519.pub`
     and paste what it prints into **Add SSH key**.
   - Backups: turn on (about 20% extra, roughly $1 a month). It's the only
     copy of your chat and files once the PC is out of the picture.
   - Name: `tuffcord`. **Create & Buy now**.
3. Note the server's **IPv4 address** (e.g. `5.161.12.34`).

## 3. Point the address at the server

In Cloudflare: your domain → **DNS → Records → Add record**:

| Type | Name | IPv4 address | Proxy status |
|------|------|--------------|--------------|
| A    | chat | the server's IPv4 | **DNS only** (grey cloud) |

"DNS only" matters: Cloudflare's proxy would sit in the middle of every voice
packet and limit uploads. It can take a few minutes to start working.

## 4. Install TUFFcord on the server

In PowerShell (use your IP and address):

```
ssh root@5.161.12.34
curl -fsSL https://raw.githubusercontent.com/tcarns/TUFFcord/main/deploy/install.sh | bash -s chat.tuffcord.net
```

It installs Caddy and the firewall (only SSH, 80 and 443 open), downloads the
latest server and checks its checksum, sets both up as services and starts
them. The last lines say the address and where settings and the log are.
Running it again is safe (it keeps settings and data).

## 5. Move your server's data over

Your current server folder on the PC has `TUFFcord-server.toml` (settings and
the group password) and `data\` (accounts, messages, files).

1. On the PC: close the server window (Ctrl+C), so nothing changes while copying.
2. On the VPS (still in ssh): `systemctl stop tuffcord` and
   `rm -rf /opt/tuffcord/data /opt/tuffcord/TUFFcord-server.toml`
3. In a **second** PowerShell window on the PC (use your server folder):
   ```
   cd "C:\path\to\your\server\folder"
   scp -r TUFFcord-server.toml data root@5.161.12.34:/opt/tuffcord/
   ```
4. Back on the VPS:
   ```
   nano /opt/tuffcord/TUFFcord-server.toml
   ```
   Set `max_storage_mb = 30000` (30 GB for files, leaving room on the 40 GB
   disk). Keep `port = 3000` (or re-run the install script after changing it).
   Ctrl+O, Enter, Ctrl+X to save.
5. ```
   chown -R tuffcord:tuffcord /opt/tuffcord
   chmod 600 /opt/tuffcord/TUFFcord-server.toml; chmod 700 /opt/tuffcord/data
   systemctl start tuffcord
   journalctl -u tuffcord -n 20
   ```
   The log should say how many older messages there are and "running on port".

## 6. Switch everyone over

In the app: sign out, type `chat.tuffcord.net` as the server address, sign in
with the same name and password as before. Tell friends the new address.

Keep the PC's server folder for a while as a fallback: starting it again
brings the old server back (with the messages as they were when you copied).

## Day to day

- **Updates**: automatic, like at home (it waits until nobody's in voice).
- **Log**: `journalctl -u tuffcord -f` (or `data/TUFFcord.log`).
- **Settings**: edit `/opt/tuffcord/TUFFcord-server.toml`, then
  `systemctl restart tuffcord`.
- **Storage**: admins see a bar in the app when sent files pass 80% of
  `max_storage_mb` or the disk is 80% full. To make room: Hetzner →
  your server → **Volumes** (about $2.50 a month per 50 GB) or **Rescale** to
  a bigger disk, then raise `max_storage_mb`.
- **Console commands** (the ones typed into the server window at home, e.g.
  `admin <name>`): admins do most of this in the app. If one is needed:
  `systemctl stop tuffcord`, then
  `cd /opt/tuffcord && sudo -u tuffcord ./TUFFcord-server`, type the command,
  Ctrl+C, `systemctl start tuffcord`.
