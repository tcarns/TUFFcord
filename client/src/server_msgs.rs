//! Messages from the server: connection events and every `ServerMsg` the app handles.

use crate::{App, Banner, Conn, Session};
use eframe::egui::{self};
use proto::{ClientMsg, ServerMsg, FEATURE_ATTACHMENTS, FEATURE_FILES};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tuffcord::net::NetEvent;

impl App {
    // ------------------------------------------------------------ network events

    pub(crate) fn handle_events(&mut self, ctx: &egui::Context) {
        while let Some(ev) = self.net.try_recv() {
            match ev {
                NetEvent::Connecting => {
                    if self.conn == Conn::Offline {
                        self.conn = Conn::Connecting;
                    }
                }
                NetEvent::Failed { message, code } => self.on_failed(message, code),
                NetEvent::Reconnecting { reason } => {
                    tuffcord::applog::warn(format!("Connection lost ({reason}); reconnecting"));
                    if let Some(ch) = self.voice_channel.clone() {
                        self.want_voice = Some(ch);
                    }
                    self.leave_voice_quietly();
                    self.conn = Conn::Reconnecting;
                    let text = match &self.server_restarting {
                        Some(v) if !v.is_empty() => {
                            format!("The server is updating to TUFFcord {v}. Back in a moment…")
                        }
                        Some(_) => "The server is restarting. Back in a moment…".to_string(),
                        None => format!("Connection lost ({reason}). Reconnecting…"),
                    };
                    self.show_banner(text, false, None);
                }
                NetEvent::Ping(ms) => self.ping = Some(ms),
                NetEvent::Attachment { id, bytes } => self.att.on_data(id, bytes),
                NetEvent::Server(msg) => self.handle_server(ctx, msg),
            }
        }
    }

    pub(crate) fn handle_server(&mut self, ctx: &egui::Context, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome {
                id,
                name,
                app_name,
                text_channels,
                voice_channels,
                max_per_voice_channel,
                history,
                voice_state,
                users,
                features,
                max_attachment_bytes,
                account,
                token,
                must_change_password,
                file_key,
                members,
            } => {
                let first = self.session.is_none();
                self.on_welcome(account, token, must_change_password);
                if let Some(r) = self.crash_report.take() {
                    self.net.send(ClientMsg::Report {
                        kind: "error".into(),
                        message: r,
                    });
                }
                self.server_restarting = None;
                self.att.server_supports = features.iter().any(|f| f == FEATURE_ATTACHMENTS);
                let endpoint = file_key
                    .filter(|_| features.iter().any(|f| f == FEATURE_FILES))
                    .map(|key| tuffcord::files::Endpoint {
                        base: proto::files::http_base(
                            &tuffcord::net::normalize_server(&self.s.server).unwrap_or_default(),
                        ),
                        key,
                    });
                if first
                    || self.att.endpoint.as_ref().map(|e| &e.base)
                        != endpoint.as_ref().map(|e| &e.base)
                {
                    self.att.reset_for_server();
                }
                tuffcord::applog::info(format!(
                    "Signed in to {} as {name}. Files: {}",
                    self.s.server,
                    match &endpoint {
                        Some(e) => format!(
                            "any kind, up to {}, via {}",
                            proto::files::size_label(max_attachment_bytes),
                            e.base
                        ),
                        None if self.att.server_supports =>
                            "images only (the server is older than 0.7)".to_string(),
                        None => "none (the server is too old)".to_string(),
                    }
                ));
                self.avatars.reset(endpoint.clone(), &members);
                self.att.endpoint = endpoint;
                if max_attachment_bytes > 0 {
                    self.att.max_bytes = max_attachment_bytes;
                }
                if !text_channels.contains(&self.current_text) {
                    self.current_text = self
                        .s
                        .last_text_channel
                        .clone()
                        .filter(|c| text_channels.contains(c))
                        .unwrap_or_else(|| text_channels[0].clone());
                }
                self.session = Some(Session {
                    me_id: id,
                    me_name: name,
                    app_name,
                    text_channels,
                    voice_channels,
                    max_per_voice: max_per_voice_channel,
                    history,
                    voice_state,
                    users,
                    members,
                    add_channels: features.iter().any(|f| f == proto::FEATURE_CHANNELS),
                    edit_channels: features.iter().any(|f| f == proto::FEATURE_CHANNEL_EDIT),
                    older: crate::history_ui::Older::new(
                        features.iter().any(|f| f == proto::FEATURE_OLDER_MESSAGES),
                    ),
                    storage_warning: None,
                    storage_hidden: false,
                    security: crate::security_ui::Security::new(&features),
                });
                self.conn = Conn::Online;
                self.login_error = None;
                self.banner = None;
                // Said at start (updated, crashed last time): still worth seeing.
                if let Some((text, error, secs)) = self.startup_notice.take() {
                    self.banner = Some(Banner {
                        text,
                        error,
                        until: secs.map(|s| Instant::now() + Duration::from_secs(s)),
                    });
                }
                if first {
                    self.focus_composer = true;
                }
                self.s.auto_connect = self.s.remember;
                self.s.save();
                self.follow_renames();
                if let Some(ch) = self.want_voice.take() {
                    self.join_voice(ch);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                    self.session
                        .as_ref()
                        .map(|s| s.app_name.clone())
                        .unwrap_or_default(),
                ));
            }
            ServerMsg::Chat { message } => {
                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                let mine = message.author_id == sess.me_id.to_string();
                let channel = message.channel.clone();
                sess.history
                    .entry(channel.clone())
                    .or_default()
                    .push(message);
                // The chat sticks to the bottom one frame later; make sure that frame happens.
                ctx.request_repaint();
                if channel != self.current_text && !mine {
                    self.unread.insert(channel);
                }
                let focused = ctx.input(|i| i.focused);
                if !mine && !focused {
                    self.sound("message");
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Informational,
                    ));
                }
            }
            ServerMsg::VoiceState { state } => {
                let Some(sess) = self.session.as_mut() else {
                    return;
                };
                if let Some(ch) = &self.voice_channel {
                    let before: HashSet<u32> = sess.members(ch).iter().map(|m| m.id).collect();
                    sess.voice_state = state;
                    let after: HashSet<u32> = sess.members(ch).iter().map(|m| m.id).collect();
                    if !after.contains(&sess.me_id) {
                        // The server no longer has us in voice.
                        self.leave_voice_quietly();
                    } else {
                        let me = sess.me_id;
                        if after.iter().any(|id| *id != me && !before.contains(id)) {
                            self.sound("join");
                        }
                        if before.iter().any(|id| *id != me && !after.contains(id)) {
                            self.sound("leave");
                        }
                    }
                } else {
                    sess.voice_state = state;
                }
                self.apply_gains();
            }
            ServerMsg::Presence { users } => {
                if let Some(sess) = self.session.as_mut() {
                    sess.users = users;
                }
                self.follow_renames();
            }
            ServerMsg::VoiceJoined { channel } => {
                // Already there: the channel was renamed (servers send this for older apps).
                if self.voice_channel.as_deref() == Some(channel.as_str()) {
                    return;
                }
                self.joining = None;
                self.voice_channel = Some(channel);
                self.ctl.in_voice.store(true, Ordering::Relaxed);
                self.audio_linger = None;
                self.sync_audio();
                self.sound("join");
                self.apply_gains();
            }
            ServerMsg::Error { code, message } => {
                if code == "voice_full" {
                    self.joining = None;
                }
                if code == "channel_failed" {
                    self.new_channel.awaiting_text = false;
                }
                if matches!(
                    code.as_str(),
                    "bad_old_password" | "weak_password_change" | "bad_rename"
                ) {
                    self.acct.form_msg = Some((message.clone(), true));
                }
                self.show_banner(message, true, Some(5));
            }
            ServerMsg::Pong { .. } => {}
            ServerMsg::AttachmentGone { id } => {
                // Cleared by the server (storage limit, or aged out): show it as expired.
                if let Some(sess) = self.session.as_mut() {
                    for a in sess
                        .history
                        .values_mut()
                        .flatten()
                        .flat_map(|m| &mut m.attachments)
                    {
                        if a.id == id {
                            a.expired = true;
                        }
                        if a.poster.as_ref().is_some_and(|p| p.id == id) {
                            a.poster = None;
                        }
                    }
                }
                if self.player.as_ref().is_some_and(|p| p.id == id) {
                    self.player = None;
                }
                self.att.gone(&id);
            }
            ServerMsg::Members { list } => {
                self.avatars.set_members(&list);
                if let Some(sess) = self.session.as_mut() {
                    sess.members = list;
                }
            }
            ServerMsg::MessageDeleted { channel, id } => {
                let mut gone = Vec::new();
                if let Some(list) = self
                    .session
                    .as_mut()
                    .and_then(|s| s.history.get_mut(&channel))
                {
                    if let Some(pos) = list.iter().position(|m| m.id == id) {
                        let m = list.remove(pos);
                        for a in m.attachments {
                            gone.extend(a.poster.map(|p| p.id));
                            gone.push(a.id);
                        }
                    }
                }
                for f in gone {
                    if self.player.as_ref().is_some_and(|p| p.id == f) {
                        self.player = None;
                    }
                    if self.confirm_open.as_ref().is_some_and(|a| a.id == f) {
                        self.confirm_open = None;
                    }
                    self.att.gone(&f); // also closes the full-size viewer
                }
                if self.confirm_delete.as_ref().is_some_and(|m| m.id == id) {
                    self.confirm_delete = None;
                }
            }
            ServerMsg::Channels {
                text_channels,
                voice_channels,
                renamed,
            } => self.on_channels(text_channels, voice_channels, renamed),
            ServerMsg::Restarting { version } => self.server_restarting = Some(version),
            ServerMsg::OlderMessages {
                channel,
                messages,
                start,
            } => self.on_older(ctx, channel, messages, start),
            ServerMsg::StorageWarning { text, critical } => {
                if let Some(sess) = self.session.as_mut() {
                    sess.storage_warning = (!text.is_empty()).then_some((text, critical));
                    sess.storage_hidden = false;
                }
            }
            // Servers before 0.12 made invites for admins in the app; now only
            // the server console does.
            ServerMsg::Invite { .. } => {}
            m @ (ServerMsg::Sessions { .. } | ServerMsg::AdminLog { .. }) => {
                self.on_security_msg(m);
            }
            m @ (ServerMsg::AccountUpdated { .. }
            | ServerMsg::Accounts { .. }
            | ServerMsg::AdminResult { .. }
            | ServerMsg::Notice { .. }) => {
                self.on_account_msg(&m);
            }
        }
    }
}
