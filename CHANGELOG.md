# Changelog

Each version's section becomes the notes on its GitHub release, which the app shows when an update is available.

## 0.11.1

Security update.

- Updates are now signed by the owner on his own computer, with a passphrase, for every release. The signing key is no longer kept on GitHub at all, so neither a stolen GitHub login nor a tampered build tool can sign an update.
- The app only tells you about an update once its signature checks out. An unsigned release no longer shows a Download button, and an old release can't be passed off as a new one.
- Deleted text channels go to the server's trash for 30 days instead of being erased, messages and files included. The server window's `trash` and `restore <n>` bring one back.

## 0.11.0

Security update. Update your server too: the new account features need it.

- Updates are signed. The app and the server install an update only if it carries TUFFcord's signature, so a stolen GitHub login can't push a bad update to everyone. Servers also wait a day before installing an update by themselves (type `update` in the server window to install at once).
- Your saved sign-in is locked to your Windows account, so another program or person copying your settings file can't use it to sign in as you.
- Admins can make one-time invites (Settings > Account > Invites). An invite creates one account in place of the group password and works for a week, even with sign-ups closed.
- Settings > Account lists where you're signed in, with a Sign out button for each device and "Sign out everywhere else".
- When your account signs in from a new address, your other devices show a notice (or the next one to connect does).
- Admins can see what admins did recently (kicks, bans, password resets, channel and invite changes) under Settings > Account > Admin log.
- Names that mix alphabets (a "Tyler" with a Cyrillic e) are refused, so nobody can pose as someone else.
- The sign-in screen warns when a server on the internet is reached without encryption.

## 0.10.1

Security update. Update your server too: most of the protection is in it.

- The server is much harder to attack from the internet: it limits how many password checks run at once and how many connections strangers can open, so it can't be run out of memory, and guesses sent all at once still count toward the wrong-password limit.
- Too many wrong passwords for one account (from any addresses) pauses sign-ins to it for an hour, except from the computer that last signed in to it.
- New passwords need at least 8 characters. Existing passwords keep working.
- The group password alone no longer lets very old apps (from before accounts) read the chat. You need an account.
- The admin who set up the server (the first admin) is now its owner. Other admins can't remove the owner's admin, reset the owner's password, or kick, ban or delete any admin. Only the owner can make or remove admins. The server window's `owner <name>` hands it on.
- Each person can send up to 3 GB of files a day (server setting `max_upload_mb_per_day`), so one account can't push everyone's older files out.
- Very large images someone sends are no longer opened, so they can't make your app use a lot of memory. More kinds of program files (.iso, .img, .vhd, .chm and others) ask before opening, and file names can't hide their real type with right-to-left text.

## 0.10.0

- Chat history is kept for good. Scroll to the top of a channel and older messages load in; "This is the start of #channel" shows when there's nothing older. Files in older messages stay too. (Needs the server to be 0.10.0. Older servers keep only the last 300 messages per channel, as before.)
- Admins see a bar above the chat when the server is running low on room for sent files (from 80% of its limit, or when its disk is 80% full), so there's time to add space before old files are deleted.
- The server can now run on a rented Linux machine, so it doesn't have to stay on someone's PC. It updates itself there too. Setup guide: docs/hosting.md on GitHub.

## 0.9.3

- Videos show a picture from the clip before you press Play instead of a black box. If the sender's app picked a black frame (a clip that fades in), TUFFcord now looks a little further in. Older videos without a picture get one once they're downloaded (clips up to 50 MB download by themselves when they show up); it's kept on your computer.
- Middle-click in the chat to scroll automatically, like in a web browser: move the mouse above or below the marker to scroll that way, faster the further you go. Click, press Esc or middle-click again to stop. (Off when the middle mouse button is your push-to-talk key.)
- The mute, deafen and settings buttons in the bottom left light up softly when you point at them.

## 0.9.2

- Make the channel list or the member list wider or narrower: drag the line between it and the chat. TUFFcord remembers the widths on your computer.
- Admins can right-click a channel to rename or delete it. Renaming keeps its messages, and anyone in a renamed voice channel stays connected. Deleting asks first; a deleted text channel's messages and files are gone for everyone.
- Admins can drag channels up and down to change their order for everyone.
- The server needs 0.9.2 for renaming, deleting and reordering.

## 0.9.1

- Admins can add channels: click the + next to "Text channels" or "Voice channels", type a name and press Enter. Everyone sees the new channel right away, and it stays after the server restarts. (The server needs 0.9.1 too.)
- The member list slides open and shut when you click the people button.

## 0.9.0

- Videos and audio play right away: clips up to 50 MB start downloading as soon as they show up in the chat, so pressing Play doesn't wait for a download. Change the size (or turn it off) in Settings, Appearance and videos, "Download videos and audio ahead of time". Bigger files still download when you press Play.
- Settings shows how much space downloaded files use, and lets you pick how much they may use (2 GB unless you change it). The oldest files are removed when it's full.
- TUFFcord opens at the same size and place you left it, also after installing an update.
- The Settings window can be moved: drag it by its title.

## 0.8.6

- Settings is split into categories down the left side: Voice and audio, Appearance and videos, Account, and Updates and about. Click one to see its settings.
- New "Default video and audio volume" setting (under Voice and audio), set to 50%. Every video or audio file in the chat starts at that volume; the speaker button on a player changes the volume of that one only.
- "Play videos here again" is always shown now (greyed out when there's nothing to undo). Hover it to see what it does.

## 0.8.5

- Videos that froze the app before 0.8.3 play in the chat again instead of opening in your video player, and the graphics card is used for videos again. Both were turned on by the freeze, not by a real problem with the video.
- Settings has a "Play videos here again" button whenever some files are set to open in your video player.

## 0.8.4

- The last of the old name is gone. The app and server still called `backroom.exe` and `backroom-server.exe` (they kept that name when updating) rename themselves to `TUFFcord.exe` and `TUFFcord-server.exe` the next time they start. A shortcut or taskbar pin to the old name may need making again, and Windows may ask once more to let `TUFFcord-server.exe` through the firewall (click Allow).
- The server's log `data\backroom.log` becomes `data\TUFFcord.log` (it carries on in the same file).

## 0.8.3

- Fixed the app freezing (and taking Windows' volume mixer with it) a few seconds into a video. It wasn't the graphics card: the video player and Windows could end up waiting on each other forever. If 0.8.2 turned off "Use the graphics card for videos" after a freeze, you can turn it back on in Settings.

## 0.8.2

- Pointing at a chat message now gives it a soft highlight, like Discord.
- On videos and audio, the volume bar is now a vertical bar that pops up above the speaker button when you point at it. Click the speaker to mute, or scroll over it, as before.

## 0.8.1

- If the app crashes, the reason now goes in its log, and the server's log gets it the next time the app starts (before, the app just closed and nothing was written down).
- If the app closes in the middle of a video, it stops using the graphics card for videos and says so; if a video still closes it after that, that file opens in your video player instead. Settings → "Use the graphics card for videos" turns the graphics card back on.
- The log now says how each video is played (on the graphics card, and which one, or without it) and its size.
- The "Updated to…" message now stays up after signing back in.

## 0.8.0

- Backroom is now **TUFFcord**. Your settings, sign-in, chat history and accounts carry over, and both the app and the server update themselves as usual. Copies that update keep their old file names (`backroom.exe`, `backroom-server.exe`), so shortcuts keep working. On the server, `backroom-server.toml` is renamed to `TUFFcord-server.toml`, and an `app_name` still at the old default becomes "TUFFcord".
- Videos and audio have a volume slider next to the speaker button. Click the speaker to mute, or scroll over it. They now start much quieter (16%, a fifth of before), and remember the volume you pick.
- Admins can delete messages: hold Shift and point at a message, click the trash can, then confirm. It's removed for everyone, along with its files, and the server log notes who deleted it. The server needs this update too.
- Fixed the app asking you to sign in again after updating itself (when "Keep me signed in" had signed you in). This already applies to the update to 0.8.0.

## 0.7.1

- Fixed sending files and showing images through a Cloudflare address ("Couldn't reach the server (native-tls: unable to find any user-specified roots…)"). File transfers now trust the same certificates Windows does, like the chat connection.
- Fixed uploads being refused when they come through Cloudflare's tunnel ("Missing or too long request"). The server needs this update too; type `update` in its window to get it right away.
- If an image can't be fetched the new way, the app asks for it over the chat connection instead, so it still shows.
- The app now keeps a log: Settings → Open log file. File problems people run into also show up in the server's log.

## 0.7.0

- Member list on the right, like Discord: who's online (and in which voice channel), then everyone else. The people button at the top of the chat hides it.
- Deafened people now show the muted mark too.
- Send any file, up to 100 MB: videos and audio play right in the chat, GIFs animate, and other files have Open and Save buttons. Files travel separately from voice, so big ones never make voices stutter.
- Color emoji (Twemoji, the set Discord uses), shown big when a message is only emoji. The picker has every emoji, with categories, search and recently used, and `:codes:` work for all of them.
- The server keeps up to 5 GB of files and clears the oldest when it's full (`max_storage_mb`).

## 0.6.0

- Accounts: everyone signs in with their own name and password, and nobody else can use their name. The group password is now only needed once, to create an account. If you used Backroom before, the app fills in your name and the group password; just pick a password.
- "Keep me signed in" saves a sign-in key instead of a password.
- Rename yourself or change your password in Settings.
- Admins can kick, ban and reset passwords: click someone's name, or use Settings → People. To become the first admin, type `admin <your name>` in the server window.
- Older apps can't sign in to this server. They're asked to update.
- Bans and the wrong-password limit go by each person's real internet address: only a proxy on the server's own PC (like the Cloudflare tunnel) can pass one along.
- Fixed the Settings window stretching across the screen when the update check failed (for example, when offline).

## 0.5.0

- The server updates itself: it downloads and checks the new version, then restarts once nobody is in voice. The app shows "The server is updating…" and reconnects on its own. Type `update` in the server window to do it right away. (This one server update, from 0.4.0 or earlier, still has to be done by hand.)
- If the server ever crashes, it starts itself again.

## 0.4.0

- Update from inside the app: click **Update now** and Backroom downloads the new version, checks it, installs it and restarts, putting you back in your voice channel. (This one update, from 0.3.0, still has to be done by hand.)

## 0.3.0

- Send images: click +, drag one onto the window, or paste with Ctrl+V. Big photos are shrunk before sending; click an image to see it full size.
- Emoji: `:)` `<3` `:D` and codes like `:fire:` turn into emoji when sent, and there's an emoji picker in the message box.
- Themes: Plum, Midnight, Light, Cream, Pastel pink and Mint, under Settings.
- Fixed dark text on a dark background on PCs that use Windows' light mode.

## 0.2.0

First native release, replacing the browser version.

- Windows app and server written in Rust.
- Voice goes through the server, so a friend's router can't block a direct connection.
- Noise suppression, voice activation sensitivity, and push to talk that works while a game is in front (keys or mouse buttons 4/5).
- Per-person volume up to 200%, and mute for me.
- Server logging with live level changes (`level`, `status`, `help`).
- The app and server tell you when a new version is available.
