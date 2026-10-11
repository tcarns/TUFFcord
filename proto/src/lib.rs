//! Wire protocol shared by the TUFFcord server and client.
//!
//! One WebSocket connection carries:
//! - Control messages as JSON text frames (`ClientMsg` / `ServerMsg`).
//! - Voice as binary frames carrying one 20 ms Opus packet each.
//! - Image uploads and downloads for apps before 0.7 (binary kinds 3 and 4).
//!
//! From 0.7, files go over plain HTTP on the same port instead (see [`files`]).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod files;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SAMPLE_RATE: u32 = 48_000;
/// 20 ms of mono audio at 48 kHz.
pub const FRAME_SAMPLES: usize = 960;
/// Largest packet Opus can produce.
pub const MAX_OPUS_PACKET: usize = 1275;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientMsg {
    /// First message on a connection. Older apps send only `name`, `password`
    /// (the group password) and `version`; servers from 0.5 and earlier ignore
    /// the rest and treat it that way.
    Hello {
        /// Account name (or, for older servers, the name to show).
        name: String,
        /// The account's password for `Login`; the group password for `Register`
        /// and for older apps.
        password: String,
        #[serde(default)]
        version: u32,
        /// The app knows about accounts (so the server can ask it to create one).
        #[serde(default)]
        accounts: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<Auth>,
    },
    /// Change your own password. Signs out your other devices.
    ChangePassword {
        old_password: String,
        new_password: String,
    },
    /// Change your account name.
    Rename {
        name: String,
    },
    /// Sign out on this device (forgets its saved sign-in on the server).
    SignOut,
    /// Admins only.
    Admin {
        action: AdminAction,
    },
    Chat {
        channel: String,
        text: String,
    },
    JoinVoice {
        channel: String,
        #[serde(default)]
        muted: bool,
        #[serde(default)]
        deafened: bool,
    },
    LeaveVoice,
    Status {
        muted: bool,
        deafened: bool,
    },
    Ping {
        t: u64,
    },
    /// Something only the client can see (audio device trouble, an app error),
    /// sent so it shows up in the server log.
    Report {
        kind: String,
        #[serde(default)]
        message: String,
    },
    /// Ask for an attachment's bytes; the server answers with an ATTACH_DATA frame.
    /// (Apps before 0.7; newer ones download over HTTP.)
    GetAttachment {
        id: String,
    },
    /// Admins: delete a message (and its files) for everyone.
    DeleteMessage {
        channel: String,
        id: String,
    },
    /// Post a message with files uploaded over HTTP (see [`files`]).
    Post {
        channel: String,
        #[serde(default)]
        text: String,
        files: Vec<PostFile>,
    },
    /// Admins: add a text channel (or a voice channel when `voice`). Servers
    /// with [`FEATURE_CHANNELS`] only.
    CreateChannel {
        name: String,
        #[serde(default)]
        voice: bool,
    },
    /// Admins: rename a channel. Servers with [`FEATURE_CHANNEL_EDIT`] only.
    RenameChannel {
        name: String,
        to: String,
        #[serde(default)]
        voice: bool,
    },
    /// Admins: delete a channel (a text channel's messages and files too).
    /// Servers with [`FEATURE_CHANNEL_EDIT`] only.
    DeleteChannel {
        name: String,
        #[serde(default)]
        voice: bool,
    },
    /// Admins: move a channel to `index` in its list. Servers with
    /// [`FEATURE_CHANNEL_EDIT`] only.
    MoveChannel {
        name: String,
        #[serde(default)]
        voice: bool,
        index: usize,
    },
    /// Messages older than the ones already shown. `before` is the `start` of
    /// the last [`ServerMsg::OlderMessages`] for this channel (leave it out the
    /// first time). Servers with [`FEATURE_OLDER_MESSAGES`] only.
    LoadOlder {
        channel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
    },
    /// Admins: make a one-time invite code (answered with
    /// [`ServerMsg::Invite`]). It works in place of the group password when
    /// creating an account. Servers with [`FEATURE_INVITES`] only.
    CreateInvite,
    /// Your signed-in devices (answered with [`ServerMsg::Sessions`]).
    /// Servers with [`FEATURE_SESSIONS`] only.
    Sessions,
    /// Sign out one of your devices (`id` from [`SessionInfo`]), or every
    /// device but this one when `id` is left out.
    EndSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Admins: recent admin actions (answered with [`ServerMsg::AdminLog`]).
    /// Servers with [`FEATURE_ADMIN_LOG`] only.
    AdminLog,
}

/// A finished upload to attach to a message, with what the sender's app
/// worked out about it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PostFile {
    pub upload: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Videos and audio.
    #[serde(default)]
    pub duration_ms: u64,
    /// Videos: an uploaded JPEG of one frame, shown before it plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
}

/// How a `Hello` signs in.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Auth {
    /// `name` and `password` are the account's.
    Login,
    /// Create an account named `name` with `new_password`; `password` is the
    /// group password, which works as the invite.
    Register { new_password: String },
    /// A sign-in the server handed out earlier (in `Welcome`).
    Token { token: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "do", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AdminAction {
    /// Disconnect them (they can sign back in).
    Kick {
        account: u32,
    },
    /// Disconnect them and stop them signing in or creating another account.
    Ban {
        account: u32,
    },
    Unban {
        account: u32,
    },
    /// Give them a temporary password (sent back in `AdminResult`) that they
    /// must change when they sign in.
    ResetPassword {
        account: u32,
    },
    SetAdmin {
        account: u32,
        admin: bool,
    },
}

/// You, as the server sees you.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub admin: bool,
}

/// One row of the admin's account list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: u32,
    pub name: String,
    pub admin: bool,
    pub banned: bool,
    pub online: bool,
    /// Milliseconds since the Unix epoch; 0 if never.
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub channel: String,
    pub author: String,
    pub author_id: String,
    pub text: String,
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub name: String,
    /// What the server found the file to be (never taken from the sender).
    pub mime: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Videos and audio.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<Poster>,
    /// Deleted to make room (the server's storage limit).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expired: bool,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A video's preview frame (a JPEG attachment of its own).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Poster {
    pub id: String,
    pub width: u32,
    pub height: u32,
}

/// Someone with an account, online or not (for the member list).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub account: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin: bool,
    /// Their profile picture's hash (fetch `GET /avatars/<account>` when it
    /// changes); none means initials. Servers before 0.12.1 leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
}

/// Sent ahead of the image bytes in an upload frame.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UploadHeader {
    pub channel: String,
    #[serde(default)]
    pub text: String,
    pub name: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceMember {
    pub id: u32,
    pub name: String,
    pub muted: bool,
    pub deafened: bool,
    /// Their account (servers without accounts leave it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VoiceChannelState {
    pub name: String,
    pub members: Vec<VoiceMember>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct User {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerMsg {
    Welcome {
        id: u32,
        name: String,
        app_name: String,
        text_channels: Vec<String>,
        voice_channels: Vec<String>,
        max_per_voice_channel: usize,
        history: BTreeMap<String, Vec<ChatMessage>>,
        voice_state: Vec<VoiceChannelState>,
        users: Vec<User>,
        /// What this server supports beyond the basics, e.g. "attachments".
        /// Older servers leave it out.
        #[serde(default)]
        features: Vec<String>,
        #[serde(default)]
        max_attachment_bytes: u64,
        /// Who you're signed in as (servers without accounts leave it out).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<Account>,
        /// A sign-in to reconnect with, and to save if "remember me" is on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Signed in with a temporary password: ask for a new one.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        must_change_password: bool,
        /// For file transfers over HTTP (servers with the "files" feature).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_key: Option<String>,
        /// Everyone with an account, for the member list.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<Member>,
    },
    Error {
        code: String,
        message: String,
    },
    Chat {
        message: ChatMessage,
    },
    VoiceState {
        state: Vec<VoiceChannelState>,
    },
    Presence {
        users: Vec<User>,
    },
    VoiceJoined {
        channel: String,
    },
    Pong {
        t: u64,
    },
    /// The attachment no longer exists (its message aged out of history).
    AttachmentGone {
        id: String,
    },
    /// The server is about to restart (e.g. to finish updating to `version`).
    /// Clients reconnect on their own; older ones ignore this message.
    Restarting {
        #[serde(default)]
        version: String,
    },
    /// Your account changed (renamed, new password). `token` replaces the old
    /// sign-in when present.
    AccountUpdated {
        account: Account,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Admins: everyone's accounts (sent after sign-in and whenever one changes).
    Accounts {
        list: Vec<AccountSummary>,
    },
    /// Admins: what an action did. `secret` is a temporary password to pass on.
    AdminResult {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
    /// Something worked (e.g. "Password changed").
    Notice {
        text: String,
    },
    /// The member list changed (someone joined, was renamed, removed…).
    Members {
        list: Vec<Member>,
    },
    /// An admin deleted a message.
    MessageDeleted {
        channel: String,
        id: String,
    },
    /// The channel lists changed (an admin added, renamed, deleted or moved one).
    Channels {
        text_channels: Vec<String>,
        voice_channels: Vec<String>,
        /// Set when a channel was renamed, so apps carry its messages over.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renamed: Option<ChannelRenamed>,
    },
    /// Answer to [`ClientMsg::LoadOlder`], oldest first. May hold messages the
    /// app already has (skip those by id). `start` is where the next page
    /// ends; 0 means there's nothing older.
    OlderMessages {
        channel: String,
        messages: Vec<ChatMessage>,
        start: u64,
    },
    /// Admins: the server is running out of room for files. Shown until a
    /// new one arrives; an empty `text` means it's fine again.
    StorageWarning {
        text: String,
        #[serde(default)]
        critical: bool,
    },
    /// Answer to [`ClientMsg::CreateInvite`]: the code (shown once) and when
    /// it stops working (ms since 1970).
    Invite {
        code: String,
        expires: u64,
    },
    /// Answer to [`ClientMsg::Sessions`], most recently used first.
    Sessions {
        list: Vec<SessionInfo>,
    },
    /// Answer to [`ClientMsg::AdminLog`], newest first.
    AdminLog {
        entries: Vec<AdminLogEntry>,
    },
}

/// One signed-in device (a saved sign-in on the server).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub created: u64,
    pub last_used: u64,
    /// Address it was last used from.
    #[serde(default)]
    pub ip: String,
    /// The device asking.
    #[serde(default)]
    pub current: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdminLogEntry {
    pub ts: u64,
    pub text: String,
}

/// A channel's old and new name (see [`ServerMsg::Channels`]).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRenamed {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub voice: bool,
}

/// The server has accounts.
pub const FEATURE_ACCOUNTS: &str = "accounts";

/// Error codes after which the app stops trying and shows the sign-in screen.
pub const FATAL_ERRORS: &[&str] = &[
    "bad_password",
    "bad_version",
    "bad_name",
    "bad_login",
    "account_required",
    "name_taken",
    "weak_password",
    "signup_closed",
    "session_expired",
    "too_many_attempts",
    "kicked",
    "banned",
    "password_reset",
];

pub const FEATURE_ATTACHMENTS: &str = "attachments";
/// Any file, over HTTP (see [`files`]).
pub const FEATURE_FILES: &str = "files";
/// Admins can add channels from the app ([`ClientMsg::CreateChannel`]).
pub const FEATURE_CHANNELS: &str = "channels";
/// Admins can rename, delete and reorder channels ([`ClientMsg::RenameChannel`],
/// [`ClientMsg::DeleteChannel`], [`ClientMsg::MoveChannel`]).
pub const FEATURE_CHANNEL_EDIT: &str = "channelEdit";
/// Keeps every message; apps load older ones with [`ClientMsg::LoadOlder`] (0.10).
pub const FEATURE_OLDER_MESSAGES: &str = "olderMessages";
/// One-time invites ([`ClientMsg::CreateInvite`]).
pub const FEATURE_INVITES: &str = "invites";
/// Signed-in devices ([`ClientMsg::Sessions`], [`ClientMsg::EndSession`]).
pub const FEATURE_SESSIONS: &str = "sessions";
/// The admin log ([`ClientMsg::AdminLog`]).
pub const FEATURE_ADMIN_LOG: &str = "adminLog";

// ---------------------------------------------------------------- voice frames

/// A 20 ms Opus packet.
pub const VOICE_DATA: u8 = 1;
/// "I stopped talking." Lets listeners stop their speaking indicator right away.
pub const VOICE_END: u8 = 2;

/// Client to server: `[kind u8][seq u32 LE][opus payload]`
pub fn encode_client_voice(kind: u8, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.push(kind);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn parse_client_voice(frame: &[u8]) -> Option<(u8, u32, &[u8])> {
    if frame.len() < 5 {
        return None;
    }
    let kind = frame[0];
    if kind != VOICE_DATA && kind != VOICE_END {
        return None;
    }
    let seq = u32::from_le_bytes(frame[1..5].try_into().ok()?);
    let payload = &frame[5..];
    if payload.len() > MAX_OPUS_PACKET || (kind == VOICE_DATA && payload.is_empty()) {
        return None;
    }
    Some((kind, seq, payload))
}

/// Server to client: `[kind u8][sender u32 LE][seq u32 LE][opus payload]`
pub fn encode_server_voice(kind: u8, sender: u32, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(9 + payload.len());
    v.push(kind);
    v.extend_from_slice(&sender.to_le_bytes());
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn parse_server_voice(frame: &[u8]) -> Option<(u8, u32, u32, &[u8])> {
    if frame.len() < 9 {
        return None;
    }
    let kind = frame[0];
    let sender = u32::from_le_bytes(frame[1..5].try_into().ok()?);
    let seq = u32::from_le_bytes(frame[5..9].try_into().ok()?);
    Some((kind, sender, seq, &frame[9..]))
}

// ---------------------------------------------------------------- attachment frames

/// Client to server: `[3][header length u32 LE][UploadHeader JSON][image bytes]`
pub const ATTACH_UPLOAD: u8 = 3;
/// Server to client: `[4][id length u8][id][image bytes]`
pub const ATTACH_DATA: u8 = 4;

/// Image types the server accepts.
pub const IMAGE_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

pub fn encode_upload(header: &UploadHeader, bytes: &[u8]) -> Vec<u8> {
    let h = serde_json::to_vec(header).expect("serialize header");
    let mut v = Vec::with_capacity(5 + h.len() + bytes.len());
    v.push(ATTACH_UPLOAD);
    v.extend_from_slice(&(h.len() as u32).to_le_bytes());
    v.extend_from_slice(&h);
    v.extend_from_slice(bytes);
    v
}

pub fn parse_upload(frame: &[u8]) -> Option<(UploadHeader, &[u8])> {
    if frame.len() < 5 || frame[0] != ATTACH_UPLOAD {
        return None;
    }
    let len = u32::from_le_bytes(frame[1..5].try_into().ok()?) as usize;
    if len > 4096 || frame.len() < 5 + len {
        return None;
    }
    let header = serde_json::from_slice(&frame[5..5 + len]).ok()?;
    Some((header, &frame[5 + len..]))
}

pub fn encode_attachment_data(id: &str, bytes: &[u8]) -> Vec<u8> {
    let id = &id.as_bytes()[..id.len().min(255)];
    let mut v = Vec::with_capacity(2 + id.len() + bytes.len());
    v.push(ATTACH_DATA);
    v.push(id.len() as u8);
    v.extend_from_slice(id);
    v.extend_from_slice(bytes);
    v
}

pub fn parse_attachment_data(frame: &[u8]) -> Option<(&str, &[u8])> {
    if frame.len() < 2 || frame[0] != ATTACH_DATA {
        return None;
    }
    let n = frame[1] as usize;
    let id = std::str::from_utf8(frame.get(2..2 + n)?).ok()?;
    Some((id, &frame[2 + n..]))
}

/// What the bytes actually are, judged by their first bytes (not the file name).
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_roundtrip() {
        let c = encode_client_voice(VOICE_DATA, 7, &[1, 2, 3]);
        let (k, seq, p) = parse_client_voice(&c).unwrap();
        assert_eq!((k, seq, p), (VOICE_DATA, 7, &[1u8, 2, 3][..]));
        let s = encode_server_voice(k, 42, seq, p);
        assert_eq!(
            parse_server_voice(&s).unwrap(),
            (VOICE_DATA, 42, 7, &[1u8, 2, 3][..])
        );
        assert!(parse_client_voice(&[9, 0, 0, 0, 0, 1]).is_none());
        assert!(parse_client_voice(&[VOICE_DATA, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn attachment_frames() {
        let h = UploadHeader {
            channel: "general".into(),
            text: "look".into(),
            name: "cat.png".into(),
            mime: "image/png".into(),
            width: 4,
            height: 3,
        };
        let f = encode_upload(&h, &[1, 2, 3]);
        let (h2, b) = parse_upload(&f).unwrap();
        assert_eq!((h2, b), (h, &[1u8, 2, 3][..]));
        let d = encode_attachment_data("abc123", &[9, 8]);
        assert_eq!(
            parse_attachment_data(&d).unwrap(),
            ("abc123", &[9u8, 8][..])
        );
        assert!(parse_upload(&[ATTACH_UPLOAD, 255, 255, 0, 0]).is_none());
        assert_eq!(
            sniff_image(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]),
            Some("image/png")
        );
        assert_eq!(sniff_image(b"GIF89a..."), Some("image/gif"));
        assert_eq!(sniff_image(b"MZ\x90\x00 not an image"), None);
    }

    #[test]
    fn old_messages_still_parse() {
        // Chat history saved by 0.2.0 (and the browser version) has no attachments field.
        let old =
            r#"{"id":"1","channel":"general","author":"A","authorId":"x","text":"hi","ts":5}"#;
        let m: ChatMessage = serde_json::from_str(old).unwrap();
        assert!(m.attachments.is_empty());
        assert!(!serde_json::to_string(&m).unwrap().contains("attachments"));
    }

    #[test]
    fn json_shape() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"type":"joinVoice","channel":"Lounge"}"#).unwrap();
        assert!(matches!(m, ClientMsg::JoinVoice { muted: false, .. }));
        let s = serde_json::to_string(&ServerMsg::VoiceJoined {
            channel: "x".into(),
        })
        .unwrap();
        assert_eq!(s, r#"{"type":"voiceJoined","channel":"x"}"#);
        let s = serde_json::to_string(&ServerMsg::Error {
            code: "a".into(),
            message: "b".into(),
        })
        .unwrap();
        assert!(s.contains(r#""type":"error""#));
    }

    #[test]
    fn hello_across_versions() {
        // What 0.2–0.5 apps send still parses, as a sign-in without accounts.
        let old: ClientMsg =
            serde_json::from_str(r#"{"type":"hello","name":"Sam","password":"pw","version":1}"#)
                .unwrap();
        assert!(matches!(
            old,
            ClientMsg::Hello {
                accounts: false,
                auth: None,
                ..
            }
        ));

        // What new apps send is still readable by servers that only know the old fields.
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        enum OldClientMsg {
            Hello {
                name: String,
                password: String,
                version: u32,
            },
        }
        let new = ClientMsg::Hello {
            name: "Sam".into(),
            password: "group".into(),
            version: PROTOCOL_VERSION,
            accounts: true,
            auth: Some(Auth::Register {
                new_password: "mine".into(),
            }),
        };
        let json = serde_json::to_string(&new).unwrap();
        assert!(json.contains(r#""auth":{"kind":"register","newPassword":"mine"}"#));
        let OldClientMsg::Hello {
            name,
            password,
            version,
        } = serde_json::from_str(&json).unwrap();
        assert_eq!(
            (name.as_str(), password.as_str(), version),
            ("Sam", "group", 1)
        );

        let admin: ClientMsg = serde_json::from_str(
            r#"{"type":"admin","action":{"do":"setAdmin","account":3,"admin":true}}"#,
        )
        .unwrap();
        assert!(matches!(
            admin,
            ClientMsg::Admin {
                action: AdminAction::SetAdmin {
                    account: 3,
                    admin: true
                }
            }
        ));
    }

    #[test]
    fn welcome_from_older_servers() {
        // A 0.5 server's Welcome has no account fields.
        let json = r#"{"type":"welcome","id":4,"name":"Sam","appName":"TUFFcord","textChannels":["general"],
            "voiceChannels":["Lounge"],"maxPerVoiceChannel":8,"history":{},"voiceState":[
            {"name":"Lounge","members":[{"id":4,"name":"Sam","muted":false,"deafened":false}]}],
            "users":[{"id":4,"name":"Sam"}],"features":["attachments"],"maxAttachmentBytes":1}"#;
        let ServerMsg::Welcome {
            account,
            token,
            must_change_password,
            users,
            voice_state,
            ..
        } = serde_json::from_str(json).unwrap()
        else {
            panic!("not a welcome")
        };
        assert!(account.is_none() && token.is_none() && !must_change_password);
        assert_eq!(users[0].account, None);
        assert_eq!(voice_state[0].members[0].account, None);
    }
}

#[cfg(feature = "updates")]
pub mod update;

#[cfg(feature = "updates")]
pub mod sshsig;
