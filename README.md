# TUFFcord

**Download:** https://github.com/tcarns/TUFFcord/releases/latest. Get the `TUFFcord-…-windows.zip` file under "Assets".

Voice and text chat for your group, written in Rust. Two programs:

- **TUFFcord.exe**: the app. Everyone runs this.
- **TUFFcord-server.exe**: the server. Only the person hosting runs this.

TUFFcord was called Backroom before 0.8.0. From 0.8.4, copies still named `backroom.exe` or `backroom-server.exe` rename themselves to `TUFFcord.exe` / `TUFFcord-server.exe` when started (make shortcuts again, and allow the server through the firewall again if Windows asks), and the server's `data\backroom.log` becomes `data\TUFFcord.log`. Settings, sign-ins, chat history and accounts carry over; `backroom-server.toml` is renamed to `TUFFcord-server.toml`, and the app's `%APPDATA%\Backroom` folder becomes `%APPDATA%\TUFFcord`.

Voice goes through the server: each person sends their audio once and the server passes it on to everyone else in the channel. Nobody's router can block a direct connection, because there aren't any.

This version doesn't work with the old browser version. Everyone needs the app.

## Hosting the server

1. Put `TUFFcord-server.exe` in a folder of its own. It saves its settings, chat history and logs next to itself.
2. Double-click it. The first time, it creates `TUFFcord-server.toml` with the default settings.
3. Keep the window open while people are using TUFFcord. If Windows asks whether to allow network access, allowing it on private networks is fine. Task Manager shows two `TUFFcord-server.exe` entries: a tiny watcher and the server itself (see Updates). That's normal.
4. To let friends connect from outside your home, start a Cloudflare tunnel in PowerShell, the same as before:
   ```powershell
   cloudflared tunnel --url http://localhost:3000
   ```
5. Type `invite` in the server window for each friend. It prints a one-time invite code that creates one account and works for 24 hours (at most 10 waiting at once). Send each friend the `….trycloudflare.com` address and their own code.
6. Make one for yourself too, create your account in the app (see Joining), then type `admin <your name>` in the server window to make yourself an admin.

## Joining

1. Run `TUFFcord.exe`. The first time, Windows may say "Windows protected your PC", because the app isn't signed. Click **More info**, then **Run anyway**.
2. **Server address:** the `….trycloudflare.com` address (with or without `https://`). If you're on the computer running the server, `localhost:3000` also works.
3. The first time, choose **Create account**: pick a name and a password of your own, and paste the invite code you were sent. After that, use **Sign in** with your name and password. With "Keep me signed in" checked, TUFFcord signs you in by itself next time.

Headphones are recommended. There's no echo cancellation yet, so speakers can feed other people's voices back into your mic.

## Using it

- **Join voice:** click a voice channel. Leave with the red phone button.
- **Mute and deafen:** the mic and headphone buttons at the bottom left.
- **Someone too loud or too quiet:** click their name (in a voice channel or the member list) to set their volume (up to 200%) or mute them just for you. This is remembered.
- **Member list:** everyone with an account, on the right, like Discord. Online people come first, with the voice channel they're in and their mute and deafen marks; offline people are faded. The crown marks admins. The people button at the top right of the chat hides or shows it (it also hides itself when the window is narrow).
- **Emoji:** color emoji (Twemoji, the same set Discord uses). A message that's only emoji (up to 27) shows them big. Type shortcuts like `:)` `:D` `;)` `<3` `:P`, or any emoji's code like `:fire:` `:joy:` `:thumbsup:` `:octopus:`. They turn into emoji when you send. The ☺ button opens a picker with every emoji, by category, with search and your recently used ones; pointing at one shows its code. (While typing, emoji in the message box are still black and white.)
- **Files:** three ways to attach one:
  - click **+** in the message box
  - drag files onto the window
  - paste an image with Ctrl+V (a screenshot from Win+Shift+S works too)

  You can attach up to 10 at once and add a caption. Any kind of file works, up to 100 MB each (the server can change that). How they show up:
  - **Images** appear in the chat. Big photos are shrunk to 2048 pixels before sending. Click one to see it full size.
  - **GIFs** play in the chat.
  - **Videos** (MP4, MOV, WebM, MKV and more) show a preview frame and their length. Click to play right in the chat; pause, skip by clicking the bar, and set the volume with the slider next to the speaker (click the speaker to mute, or scroll over either to adjust). Videos and audio start quiet (16%) and remember the volume you pick. The ↗ button opens it in your usual video player. If a video can't play inside TUFFcord, it says so and offers your player instead.
  - **Audio** (MP3, WAV, OGG, FLAC, M4A…) gets a player with a seek bar and the same volume slider.
  - **Anything else** shows its name and size, with **Open** and **Save**. Opening a program or script (.exe, .bat, .ps1…) asks first, and Windows shows its usual warning, the same as for a download from a browser.

  Right-click an image, video or audio file for **Save as…**. Videos and audio play with Windows' own decoders (the ones Films & TV and Media Player use), on the graphics card when there is one. WebM needs Microsoft's VP9 extension, which most Windows 10 and 11 PCs already have. Files you open or play are kept in `%LOCALAPPDATA%\TUFFcord\cache` (up to 2 GB, oldest removed first).
- **Settings** (the sliders button):
  - **Mic and speakers:** pick devices and use the mic meter to test your microphone.
  - **When to send your voice:**
    - **When I talk:** the sensitivity slider sets how loud you need to be. Your mic opens when the meter passes the amber line.
    - **Push to talk:** any key or mouse button 4/5 works, even while a game is in front.
  - **Noise suppression:** removes background noise like fans and keyboard clicks.
  - **Join, leave and message sounds:** turn the chimes on or off.
  - **Theme:** Plum, Midnight, Light, Cream, Pastel pink or Mint. Every theme is checked for readable contrast.
  - **Memory:** shows how much memory TUFFcord is using.

The app's settings are saved in `%APPDATA%\TUFFcord\settings.json`.

## Accounts

Everyone has their own account: a name nobody else can take, and their own password. New accounts need a one-time invite, made only in the server window (`invite`; on a Linux server, `tuffcord invite`). Each invite creates one account and works for 24 hours, and at most 10 can be waiting at once. `invites` lists the waiting ones (when made, time left; codes can't be shown again) and `uninvite <n>` or `uninvite all` cancels them. Since 0.12 there is no group password.

- **Your account** (Settings, at the bottom): rename yourself, change your password, or sign out. Changing your password signs you out on your other computers.
- **Keep me signed in** saves a sign-in key from the server in `settings.json`, not your password. Signing out deletes the key on the server too, so a copied `settings.json` stops working. Keys you don't use for 180 days expire.
- **Volume and "mute for me"** follow people when they rename.

**Admins** can manage people from the app. Click someone's name (in a voice channel or under Online) for **Kick**, **Reset password** and **Ban**. Settings → People lists every account, including offline ones, with the same actions plus **Unban** and **Make admin**.
- **Kick** disconnects someone. They can sign back in.
- **Ban** disconnects them and stops them signing in. They also can't create a new account from the same internet address.
- **Reset password** signs them out everywhere and gives you a temporary password to send them. They choose a new one when they sign in.
- There's always at least one admin, and admins can't be banned until their admin is removed.
- **Delete a message:** hold Shift and point at it. A trash can appears at its top right; click it, then confirm with **Delete** (or Enter). It's removed for everyone, with any files attached to it, and the server log notes who deleted whose message. Only admins see the trash can, and the server refuses anyone else.

The same things work from the server window: `users`, `admin <name>`, `unadmin <name>`, `kick <name>`, `ban <name>`, `unban <name>`, `resetpw <name>` and `deluser <name>`. Deleting an account frees its name. Someone who forgets their password: `resetpw <name>`.

Accounts are saved in `data\accounts.json`. Passwords are stored only as Argon2 hashes, and sign-in keys only as SHA-256 hashes, so the file can't be used to sign in as anyone. Still, keep it private and include it in backups. Invite codes are stored hashed too. To stop new accounts, just don't make invites.

Ten wrong passwords from one address in ten minutes make it wait. Wrong passwords are logged.

## Server settings

Edit `TUFFcord-server.toml` next to the server, then restart it:

| Setting | Default | What it does |
| --- | --- | --- |
| `port` | `3000` | Port to listen on |
| `app_name` | `TUFFcord` | Name shown in the app. A settings file from before the rename that still said `Backroom` is switched to `TUFFcord` once. |
| `text_channels` | `["general", "links", "memes"]` | Text channels |
| `voice_channels` | `["Lounge", "Gaming", "Quiet room"]` | Voice channels |
| `max_per_voice_channel` | `8` | Most people in one voice channel |
| `history_limit` | `300` | Messages kept per text channel |
| `max_attachment_mb` | `100` | Largest file people can send, in MB |
| `max_storage_mb` | `5120` | Most space all sent files may use together. When it's full, the oldest files are deleted (their messages stay, marked as cleared). |
| `log_level` | `info` | How much to log |
| `log_file` | `data/TUFFcord.log` | Log file. `""` logs to the window only. `data/backroom.log` from before the rename is switched to this one. |
| `check_updates` | `true` | Check GitHub for a new version every 6 hours |
| `auto_update` | `true` | Install new versions by itself (see Updates). `false` only tells you, and you type `update` to install. |

Environment variables with the same names in capitals (`PASSWORD`, `PORT`, `LOG_LEVEL` and so on) override the file. These are the same names the old Node version used.

Files are saved in `data\attachments`. They're deleted automatically when their message ages out of the chat history, or when the oldest have to make room under `max_storage_mb`. If your settings file came from 0.6.0 or earlier and still had the old 8 MB image limit, 0.7.0 switches it to the new defaults (100 MB, 5 GB) once; a limit you'd changed is left alone.

Files travel over plain HTTP on the server's port, separately from voice, so a big upload or download never makes anyone's voice stutter. The Cloudflare tunnel carries both, with nothing to set up.

**Mixing versions:** 0.7 apps and servers work with 0.8 ones. A message an admin deletes disappears from 0.7 apps the next time they connect, rather than straight away, and only 0.8 apps show the trash can. With a 0.7.0 server, 0.6.0 apps still work but only show images (other files say they aren't available), and a 0.7.0 app on a 0.6.0 server can only send images. A server with accounts (0.6.0 and later) needs apps from 0.6.0 on. Older apps are told to close, reopen and click **Update now**. Apps from 0.6.0 still work with older servers, signing in with the group password. On a 0.12 server, 0.6 to 0.11 apps create an account by typing the invite code into their group password box. Between 0.2.0 and 0.5.0, any app works with any server. A 0.3.0 app on a 0.2.0 server can't send images (the + button is greyed out), and a 0.2.0 app on a 0.3.0 server shows messages without their images.

**Keeping your old chat history:** the history format is the same as the browser version's. Copy the old `data\messages.json` into the `data` folder next to `TUFFcord-server.exe` before starting it.

## Logging

**Server:** everything is logged to the server window and to the `log_file` (`data\TUFFcord.log`) next to `TUFFcord-server.exe`. When the log file passes 5 MB, it's renamed with `.1` on the end and a fresh one starts.

**App:** each person's app keeps its own log in `%APPDATA%\TUFFcord\TUFFcord.log` (Settings → **Open log file**): sign-ins, lost connections, every error the app shows, and each file sent or downloaded, with the server's exact answer when something fails. It's replaced by a fresh one past 2 MB (the previous one is kept as `TUFFcord.log.1`).

**Crashes:** if the app crashes, it writes down why before closing: the error, the part of the program (or Windows file, such as a graphics driver or video decoder) it happened in, and which video was playing. The next time it starts, it sends that to the server's log as an `App error on <name>'s computer`, so you see it without asking. If it closed in the middle of a video, it also avoids doing it again: the first time it stops using the graphics card for videos (Settings → **Use the graphics card for videos** turns it back on), and if a video still closes it without the graphics card, that file opens in the person's own video player from then on.

| Level | Shows |
| --- | --- |
| `trace` | When each person starts and stops talking |
| `debug` | Chat activity (who posted where, never what they said), mute and deafen, ignored messages, every file request |
| `info` | Sign-ins, new accounts, renames, disconnects, voice joins, leaves and moves |
| `info` / `warn` | File requests the server refused, and why (who asked, what was wrong) |
| `warn` | Wrong passwords, admin actions (kicks, bans, password resets, deleted messages), mic or speaker problems on someone's computer, audio breaking up, people sending too fast, files someone couldn't send or open |
| `error` | App errors reported by someone's computer, saving problems |
| `critical` | The server crashing or failing to start |

Type these into the server window to change things while it runs:

| Command | Does |
| --- | --- |
| `level` | Show the current level |
| `level debug` | Switch to that level right away (any level name works) |
| `status` | Who's online and in voice, uptime and the server's memory use |
| `update` | Check for a new version now, install it and restart right away |
| `users`, `admin`, `kick`, `ban`, `resetpw` … | Manage accounts (see Accounts) |
| `help` | List the commands |

The app reports problems only it can see, so they show up in your server log:
- a microphone or speakers that failed to open or got unplugged
- someone's audio breaking up repeatedly because of network delay
- a file someone couldn't send, download or play (with the reason)

## Updates

The app and server check for a new version when they start and every 6 hours after that. You can also check from the app's settings, or turn automatic checks off there. On the server, set `check_updates = false` in `TUFFcord-server.toml` (`update` still works).

**The app updates itself.** When a new version is out, a bar says "TUFFcord X is available". Click **Update now**, and the app:
1. downloads the new version, with a progress bar
2. checks it against the release's checksum file, so a broken or incomplete download is never installed
3. replaces itself and restarts, signing back in and rejoining your voice channel

If anything goes wrong, nothing is changed. The bar offers **Try again** and **Download manually**, and the full reason appears in a message.

For updating to work, `TUFFcord.exe` has to be in a folder you can save files in, like Desktop, Documents or Downloads. A folder like Program Files won't work. While updating, the old copy is briefly kept next to it as `TUFFcord.exe.old` and removed on the next start.

In-app updating started with 0.4.0, so going from 0.3.0 to 0.4.0 is done by hand: download the zip and replace the app's `.exe`.

**The server updates itself too** (from 0.5.0 on). When it finds a new version, it:
1. downloads `TUFFcord-server.exe` and checks it against the release's checksum file
2. runs it once with `--version`, to make sure it starts on this PC and is really the new version
3. swaps it in, then waits until nobody is in a voice channel and chat has been quiet for a minute
4. tells everyone it's restarting, saves chat history and restarts, which takes about a second

The apps say "The server is updating…", reconnect on their own, and rejoin voice. Settings, chat history, images and logs are kept. If any step fails, nothing is changed. The window says why, and it tries again at the next check.

Type `update` in the server window to check right now and install and restart immediately, even if people are in voice. To be asked instead, set `auto_update = false`: the window then says when a version is out, and `update` installs it.

The restart works because double-clicking `TUFFcord-server.exe` starts a small watcher, which runs the server and starts it again after an update. The watcher also restarts the server if it ever crashes (after it has been running at least 30 seconds, so a bad setting doesn't loop). Ctrl+C or closing the window stops both. Ending either one in Task Manager also ends the other.

Server updating started with 0.5.0, so getting the server from 0.4.0 or earlier to 0.5.0 is done by hand one last time: close the server, replace its `.exe` with the one from the zip, and start it again.

## Publishing a new version

1. Raise `version` in `Cargo.toml` (for example `0.2.0` to `0.2.1`).
2. Add a section for that version at the top of `CHANGELOG.md`. It becomes the release notes people see.
3. Commit and push to `main`.

GitHub notices the new version number, runs the tests, builds both programs on its own machines, and publishes them as a release tagged `v0.2.1`. This is the "Release" workflow in the Actions tab. The release includes the zip, the two `.exe` files (what the app and server download when they update themselves, plus copies named `backroom.exe` and `backroom-server.exe` for copies from before the rename, which look for those names) and `SHA256SUMS.txt`, the checksums the app verifies against. Pushes that don't change the version just run the tests. Running copies of TUFFcord notice the release on their next check.

## Credits

- Color emoji: [Twemoji](https://github.com/jdecked/twemoji) by Twitter, Inc. and other contributors, licensed [CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/).
- Emoji names, groups and codes: [emojibase](https://emojibase.dev) (MIT).
- Fallback emoji font: Symbola by George Douros (free for any use).

Details are in `client/assets/NOTICE.md`.

## Building from source

On Windows:
1. Install Rust from rustup.rs.
2. Install the Visual Studio Build Tools (C++) and CMake. These are needed to compile the Opus audio codec.
3. Run `cargo build --release`. The programs land in `target\release\` as `TUFFcord.exe` and `TUFFcord-server.exe`.

To cross-compile the Windows programs from Linux, follow the steps in `.github/workflows/release.yml`.

`cargo test` runs the unit tests, and `tests/run.py` the end-to-end ones (real server, test bots, accounts, files, voice, self-update, an HTTPS tunnel like Cloudflare's; `--compat` adds checks against the previous release). GitHub runs both on every push, and the version checks before each release. The `tuffcord-bot` program is a headless test client that plays a tone and measures what it hears:

```
tuffcord-bot --server ws://localhost:3000/ws --password pw --name Bot --channel Lounge --tone 440 --listen 660 --seconds 10
```
