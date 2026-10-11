//! Channels admins add, rename, delete and reorder from the app: checked,
//! saved to the settings file and sent to everyone signed in.

use crate::{clean, config, leave_voice, trash, State};
use proto::{ChannelRenamed, ServerMsg};
use std::sync::Arc;

/// Most channels of each kind, so the sidebar stays usable.
const MAX_CHANNELS: usize = 50;
const MAX_NAME: usize = 40;

/// An admin asked for a new text channel (or voice channel when `voice`).
pub fn create(st: &mut State, conn: u32, name: &str, voice: bool) {
    if !admin(st, conn, "add channels") {
        return;
    }
    let kind = kind(voice);
    let name = clean_name(name, voice);
    let list = list(st, voice);
    if name.is_empty() {
        return refuse(st, conn, "Give the channel a name.");
    }
    if list.iter().any(|c| c.to_lowercase() == name.to_lowercase()) {
        return refuse(
            st,
            conn,
            &format!("There's already a {kind} channel called {name}."),
        );
    }
    if list.len() >= MAX_CHANNELS {
        return refuse(
            st,
            conn,
            &format!("There are already {MAX_CHANNELS} {kind} channels."),
        );
    }

    let cfg = Arc::make_mut(&mut st.cfg);
    if voice {
        cfg.voice_channels.push(name.clone());
    } else {
        cfg.text_channels.push(name.clone());
        st.history.entry(name.clone()).or_default();
    }
    save(st, voice);
    admin_log!(
        st,
        "{} added the {kind} channel {}",
        st.who(conn),
        shown(&name, voice)
    );
    send_lists(st, None);
}

/// An admin renamed a channel: messages and people in voice move with it.
pub fn rename(st: &mut State, conn: u32, name: &str, to: &str, voice: bool) {
    if !admin(st, conn, "rename channels") {
        return;
    }
    let kind = kind(voice);
    let to = clean_name(to, voice);
    let list = list(st, voice);
    let Some(pos) = list.iter().position(|c| c == name) else {
        return refuse(st, conn, "That channel isn't there any more.");
    };
    if to.is_empty() {
        return refuse(st, conn, "Give the channel a name.");
    }
    if to == name {
        return;
    }
    let taken = list
        .iter()
        .enumerate()
        .any(|(i, c)| i != pos && c.to_lowercase() == to.to_lowercase());
    if taken {
        return refuse(
            st,
            conn,
            &format!("There's already a {kind} channel called {to}."),
        );
    }

    let cfg = Arc::make_mut(&mut st.cfg);
    let mut in_voice = Vec::new();
    if voice {
        cfg.voice_channels[pos] = to.clone();
        for c in st.clients.values_mut() {
            if c.voice.as_deref() == Some(name) {
                c.voice = Some(to.clone());
                in_voice.push(c.id);
            }
        }
    } else {
        cfg.text_channels[pos] = to.clone();
        let mut messages = st.history.remove(name).unwrap_or_default();
        for m in &mut messages {
            m.channel = to.clone();
        }
        st.history.insert(to.clone(), messages);
        st.history_dirty = true;
        st.archive.rename(name, &to);
    }
    save(st, voice);
    admin_log!(
        st,
        "{} renamed the {kind} channel {} to {}",
        st.who(conn),
        shown(name, voice),
        shown(&to, voice)
    );
    send_lists(
        st,
        Some(ChannelRenamed {
            from: name.to_string(),
            to: to.clone(),
            voice,
        }),
    );
    if voice {
        // Apps from before 0.9.2 don't read `renamed`: tell them where they are now.
        for id in in_voice {
            st.send(
                id,
                &ServerMsg::VoiceJoined {
                    channel: to.clone(),
                },
            );
        }
        st.broadcast_voice();
    }
}

/// An admin deleted a channel: a text channel's messages and files go to the
/// trash (30 days), people in a voice channel are moved out of voice.
pub fn delete(st: &mut State, conn: u32, name: &str, voice: bool) {
    if !admin(st, conn, "delete channels") {
        return;
    }
    let kind = kind(voice);
    let list = list(st, voice);
    let Some(pos) = list.iter().position(|c| c == name) else {
        return resync(st, conn);
    };
    if list.len() == 1 {
        return refuse(st, conn, &format!("Keep at least one {kind} channel."));
    }

    let cfg = Arc::make_mut(&mut st.cfg);
    let mut detail = String::new();
    if voice {
        cfg.voice_channels.remove(pos);
    } else {
        cfg.text_channels.remove(pos);
        let messages = st.history.remove(name).unwrap_or_default();
        st.history_dirty = true;
        // Kept 30 days in data/trash ("restore" in the server window).
        let (count, files) = trash::keep(st, name, messages);
        detail = format!(" ({count} message(s), {files} file(s); in the trash for 30 days)");
    }
    save(st, voice);
    admin_log!(
        st,
        "{} deleted the {kind} channel {}{detail}",
        st.who(conn),
        shown(name, voice)
    );
    send_lists(st, None);
    if voice {
        let ids: Vec<u32> = st
            .clients
            .values()
            .filter(|c| c.voice.as_deref() == Some(name))
            .map(|c| c.id)
            .collect();
        for id in ids {
            leave_voice(st, id, Some("the channel was deleted"));
        }
    }
}

/// An admin dragged a channel to `index` in its list.
pub fn move_to(st: &mut State, conn: u32, name: &str, voice: bool, index: usize) {
    if !admin(st, conn, "reorder channels") {
        return resync(st, conn);
    }
    let list = list(st, voice);
    let Some(pos) = list.iter().position(|c| c == name) else {
        return resync(st, conn);
    };
    let index = index.min(list.len() - 1);
    if index == pos {
        return;
    }
    let cfg = Arc::make_mut(&mut st.cfg);
    let list = if voice {
        &mut cfg.voice_channels
    } else {
        &mut cfg.text_channels
    };
    let ch = list.remove(pos);
    list.insert(index, ch);
    save(st, voice);
    admin_log!(
        st,
        "{} moved the {} channel {} to place {}",
        st.who(conn),
        kind(voice),
        shown(name, voice),
        index + 1
    );
    send_lists(st, None);
    if voice {
        st.broadcast_voice();
    }
}

/// Check the account, not the connection: admin may have just been removed.
fn admin(st: &State, conn: u32, what: &str) -> bool {
    let ok = st
        .clients
        .get(&conn)
        .and_then(|c| c.account)
        .and_then(|a| st.accounts.get(a))
        .is_some_and(|a| a.admin);
    if !ok {
        warn!(
            "admin",
            "{} tried to {what} but isn't an admin",
            st.who(conn)
        );
        refuse(st, conn, &format!("Only admins can {what}."));
    }
    ok
}

/// A name as the server keeps it: text channels lowercase with dashes.
fn clean_name(name: &str, voice: bool) -> String {
    if voice {
        clean(name, MAX_NAME)
    } else {
        let c = config::clean_channel(name);
        let c: String = c.trim_matches('-').chars().take(MAX_NAME).collect();
        c.trim_end_matches('-').to_string()
    }
}

fn kind(voice: bool) -> &'static str {
    if voice {
        "voice"
    } else {
        "text"
    }
}

fn shown(name: &str, voice: bool) -> String {
    if voice {
        name.to_string()
    } else {
        format!("#{name}")
    }
}

fn list(st: &State, voice: bool) -> &Vec<String> {
    if voice {
        &st.cfg.voice_channels
    } else {
        &st.cfg.text_channels
    }
}

/// Write both lists into the settings file.
fn save(st: &State, voice: bool) {
    let cfg = &st.cfg;
    if let Err(e) = config::save_channels(&cfg.config_path, &cfg.text_channels, &cfg.voice_channels)
    {
        error!(
            "admin",
            "Could not save the channel list to {} ({e}); the change is gone after a restart",
            cfg.config_path.display()
        );
    }
    let env = if voice {
        "VOICE_CHANNELS"
    } else {
        "TEXT_CHANNELS"
    };
    if std::env::var_os(env).is_some_and(|v| !v.is_empty()) {
        warn!(
            "admin",
            "{env} is set, so the channel list in the settings file is ignored; the change is gone after a restart"
        );
    }
}

/// The text channel list changed outside the app (a restore): save and tell everyone.
pub(crate) fn saved_and_sent(st: &State) {
    save(st, false);
    send_lists(st, None);
}

fn send_lists(st: &State, renamed: Option<ChannelRenamed>) {
    st.broadcast(&ServerMsg::Channels {
        text_channels: st.cfg.text_channels.clone(),
        voice_channels: st.cfg.voice_channels.clone(),
        renamed,
    });
}

/// The app showed something that isn't so (it moved a channel ahead of the
/// server, or one is already gone): send it the lists as they are.
fn resync(st: &State, conn: u32) {
    st.send(
        conn,
        &ServerMsg::Channels {
            text_channels: st.cfg.text_channels.clone(),
            voice_channels: st.cfg.voice_channels.clone(),
            renamed: None,
        },
    );
}

fn refuse(st: &State, conn: u32, message: &str) {
    st.send(
        conn,
        &ServerMsg::Error {
            code: "channel_failed".into(),
            message: message.into(),
        },
    );
}
