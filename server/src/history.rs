//! Chat history. The newest `history_limit` messages per channel stay in
//! memory (saved in `data/messages.json`, sent on sign-in); older ones move to
//! `data/history/<channel>.jsonl`, one message per line, and are read a page
//! at a time when an app scrolls up. Nothing is dropped.

use crate::config::Config;
use crate::{attachments_dir, files, State};
use proto::{ChatMessage, ServerMsg};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Messages per `OlderMessages` page.
const PAGE: usize = 50;

// ---------------------------------------------------------------- recent messages

fn recent_path(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("messages.json")
}

/// Recent messages, minus any already moved to the archive (the server can
/// stop between writing the archive and saving `messages.json`).
pub fn load(cfg: &Config, archived: &HashSet<String>) -> BTreeMap<String, Vec<ChatMessage>> {
    let path = recent_path(cfg);
    let mut history: BTreeMap<String, Vec<ChatMessage>> = BTreeMap::new();
    if path.exists() {
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        {
            Ok(h) => history = h,
            Err(e) => error!(
                "chat",
                "Could not read chat history ({e}). Starting with empty history."
            ),
        }
    }
    history.retain(|ch, _| cfg.text_channels.contains(ch));
    for ch in &cfg.text_channels {
        history.entry(ch.clone()).or_default();
    }
    for list in history.values_mut() {
        list.retain(|m| !archived.contains(&m.id));
    }
    history
}

pub fn save(cfg: &Config, history: &BTreeMap<String, Vec<ChatMessage>>) {
    let path = recent_path(cfg);
    let tmp = path.with_extension("json.tmp");
    let result = std::fs::create_dir_all(&cfg.data_dir)
        .and_then(|_| std::fs::write(&tmp, serde_json::to_vec(history).unwrap()))
        .and_then(|_| std::fs::rename(&tmp, &path));
    match result {
        Ok(()) => trace!("chat", "Chat history saved"),
        Err(e) => error!("chat", "Saving chat history failed: {e}"),
    }
}

/// Add a message (moving the oldest past the limit to the archive) and send it to everyone.
pub fn post(st: &mut State, message: ChatMessage) {
    let limit = st.cfg.history_limit.max(1);
    let list = st.history.entry(message.channel.clone()).or_default();
    list.push(message.clone());
    if list.len() > limit {
        let extra = list.len() - limit;
        let old: Vec<ChatMessage> = list.drain(..extra).collect();
        st.archive.append(&old);
    }
    st.history_dirty = true;
    st.last_chat = Some(Instant::now());
    st.broadcast(&ServerMsg::Chat { message });
}

/// An app scrolled up: send the page of archived messages before `before`.
pub fn load_older(st: &State, conn: u32, channel: &str, before: Option<u64>) {
    if !st.cfg.text_channels.iter().any(|c| c == channel) {
        return;
    }
    let (messages, start) = st.archive.page(channel, before);
    st.send(
        conn,
        &ServerMsg::OlderMessages {
            channel: channel.to_string(),
            messages,
            start,
        },
    );
}

/// An admin deletes a message: gone from history for everyone, files and all.
pub fn delete_message(st: &mut State, conn: u32, channel: &str, msg_id: &str) {
    let by = st.who(conn);
    // Check the account, not the connection: admin may have just been removed.
    let admin = st
        .clients
        .get(&conn)
        .and_then(|c| c.account)
        .and_then(|a| st.accounts.get(a))
        .is_some_and(|a| a.admin);
    if !admin {
        warn!("admin", "{by} tried to delete a message but isn't an admin");
        st.send(
            conn,
            &ServerMsg::Error {
                code: "not_admin".into(),
                message: "Only admins can delete messages.".into(),
            },
        );
        return;
    }
    let Some(list) = st.history.get_mut(channel) else {
        return;
    };
    let message = match list.iter().position(|m| m.id == msg_id) {
        Some(pos) => {
            st.history_dirty = true;
            Some(list.remove(pos))
        }
        None => st.archive.delete(channel, msg_id),
    };
    let Some(message) = message else {
        // Already gone (someone else deleted it): make sure the app forgets it too.
        st.send(
            conn,
            &ServerMsg::MessageDeleted {
                channel: channel.to_string(),
                id: msg_id.to_string(),
            },
        );
        return;
    };
    let dir = attachments_dir(&st.cfg);
    let mut files_removed = 0;
    for a in &message.attachments {
        for id in std::iter::once(&a.id).chain(a.poster.as_ref().map(|p| &p.id)) {
            let freed = files::remove_file(&dir, id);
            if freed > 0 {
                files_removed += 1;
            }
            st.files.stored = st.files.stored.saturating_sub(freed);
        }
    }
    admin_log!(
        st,
        "{by} deleted a message by {} in #{channel}{}",
        message.author,
        if files_removed > 0 {
            format!(" ({files_removed} file(s) removed)")
        } else {
            String::new()
        }
    );
    st.broadcast(&ServerMsg::MessageDeleted {
        channel: channel.to_string(),
        id: msg_id.to_string(),
    });
}

/// Delete stored files no message refers to any more (and unfinished uploads).
/// Returns the bytes still stored.
pub fn cleanup_attachments(
    cfg: &Config,
    history: &BTreeMap<String, Vec<ChatMessage>>,
    archive: &Archive,
) -> u64 {
    let Ok(entries) = std::fs::read_dir(attachments_dir(cfg)) else {
        return 0;
    };
    let keep: HashSet<&str> = history
        .values()
        .flatten()
        .flat_map(|m| &m.attachments)
        .filter(|a| !a.expired)
        .flat_map(|a| {
            std::iter::once(a.id.as_str()).chain(a.poster.as_ref().map(|p| p.id.as_str()))
        })
        .chain(archive.files.keys().map(|k| k.as_str()))
        .collect();
    let mut removed = 0;
    let mut stored = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if keep.contains(name.to_string_lossy().as_ref()) {
            stored += entry.metadata().map(|m| m.len()).unwrap_or(0);
        } else if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        info!(
            "chat",
            "Removed {removed} stored files that are no longer in chat history"
        );
    }
    stored
}

// ---------------------------------------------------------------- the archive

/// A stored file whose message is in the archive.
pub struct ArchivedFile {
    pub msg: String,
    pub channel: String,
    pub ts: u64,
    pub mime: String,
    pub name: String,
    pub size: u64,
}

/// Messages older than the recent ones, one append-only file per channel.
/// Only where each line starts is kept in memory (8 bytes a message), plus
/// the files those messages still have.
pub struct Archive {
    dir: PathBuf,
    /// Per channel: where each line starts, then the file's length.
    lines: HashMap<String, Vec<u64>>,
    /// File id -> its message, for downloads and the storage limit.
    pub files: HashMap<String, ArchivedFile>,
}

/// Same length as the line it replaces (padded with spaces), so the lines
/// after it keep their place; it doesn't parse as a message.
const DELETED: &str = r#"{"deleted":true}"#;

impl Archive {
    /// Read where every line starts. Returns the ids of archived messages too,
    /// so `load` can drop copies still in `messages.json`.
    pub fn load(data_dir: &Path, channels: &[String]) -> (Archive, HashSet<String>) {
        let dir = data_dir.join("history");
        let attachments = data_dir.join("attachments");
        let mut archive = Archive {
            dir,
            lines: HashMap::new(),
            files: HashMap::new(),
        };
        let mut ids = HashSet::new();
        for ch in channels {
            ids.extend(archive.index(ch, &attachments));
        }
        let count: usize = archive.lines.values().map(|o| o.len() - 1).sum();
        if count > 0 {
            info!("chat", "{count} older message(s) in data/history");
        }
        (archive, ids)
    }

    /// Read where every line of `ch`'s file starts and which files its
    /// messages have. Returns the message ids.
    fn index(&mut self, ch: &str, attachments: &Path) -> HashSet<String> {
        let mut ids = HashSet::new();
        let path = self.path(ch);
        let mut offsets = vec![0u64];
        if let Ok(file) = std::fs::File::open(&path) {
            let mut reader = std::io::BufReader::new(file);
            let mut line = Vec::new();
            let mut pos = 0u64;
            loop {
                line.clear();
                let n = match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(n) => n as u64,
                    Err(e) => {
                        error!("chat", "Reading {} failed: {e}", path.display());
                        break;
                    }
                };
                if line.last() != Some(&b'\n') {
                    // Cut short (the server stopped mid-write): drop the partial line.
                    let _ = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .and_then(|f| f.set_len(pos));
                    break;
                }
                pos += n;
                offsets.push(pos);
                if let Ok(m) = serde_json::from_slice::<ChatMessage>(&line) {
                    self.add_files(ch, &m, Some(attachments));
                    ids.insert(m.id);
                }
            }
        }
        self.lines.insert(ch.to_string(), offsets);
        ids
    }

    fn path(&self, channel: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", file_name(channel)))
    }

    /// Remember a message's files (when `check` is given, only those still on disk).
    fn add_files(&mut self, channel: &str, m: &ChatMessage, check: Option<&Path>) {
        for a in m.attachments.iter().filter(|a| !a.expired) {
            let poster = a
                .poster
                .as_ref()
                .map(|p| (p.id.clone(), "image/jpeg", "poster.jpg", 0));
            let main = Some((a.id.clone(), a.mime.as_str(), a.name.as_str(), a.size));
            for (id, mime, name, size) in main.into_iter().chain(poster) {
                if check.is_some_and(|dir| !dir.join(&id).exists()) {
                    continue;
                }
                self.files.insert(
                    id,
                    ArchivedFile {
                        msg: m.id.clone(),
                        channel: channel.to_string(),
                        ts: m.ts,
                        mime: mime.to_string(),
                        name: name.to_string(),
                        size,
                    },
                );
            }
        }
    }

    /// Add messages that left the recent list (oldest first).
    pub fn append(&mut self, messages: &[ChatMessage]) {
        for m in messages {
            let path = self.path(&m.channel);
            let mut line = serde_json::to_vec(m).unwrap();
            line.push(b'\n');
            let written = std::fs::create_dir_all(&self.dir).and_then(|_| {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                f.write_all(&line)
            });
            if let Err(e) = written {
                error!(
                    "chat",
                    "Saving an older message to {} failed: {e}",
                    path.display()
                );
                continue;
            }
            let offsets = self
                .lines
                .entry(m.channel.clone())
                .or_insert_with(|| vec![0]);
            let end = *offsets.last().unwrap() + line.len() as u64;
            offsets.push(end);
            self.add_files(&m.channel, m, None);
        }
    }

    /// Up to a page of messages before line `before` (the end when `None`),
    /// oldest first, and the line the page starts at.
    pub fn page(&self, channel: &str, before: Option<u64>) -> (Vec<ChatMessage>, u64) {
        let Some(offsets) = self.lines.get(channel) else {
            return (Vec::new(), 0);
        };
        let count = offsets.len() - 1;
        let end = before.map_or(count, |b| (b as usize).min(count));
        let start = end.saturating_sub(PAGE);
        if start == end {
            return (Vec::new(), 0);
        }
        let (from, to) = (offsets[start], offsets[end]);
        let mut bytes = vec![0; (to - from) as usize];
        let read = std::fs::File::open(self.path(channel)).and_then(|mut f| {
            f.seek(SeekFrom::Start(from))?;
            f.read_exact(&mut bytes)
        });
        if let Err(e) = read {
            error!("chat", "Reading older messages in #{channel} failed: {e}");
            return (Vec::new(), 0);
        }
        let messages = bytes
            .split(|b| *b == b'\n')
            .filter_map(|l| serde_json::from_slice::<ChatMessage>(l).ok())
            .map(|mut m| {
                m.channel = channel.to_string();
                // Files deleted since (storage limit) show as expired.
                for a in &mut m.attachments {
                    if !self.files.contains_key(&a.id) {
                        a.expired = true;
                    }
                    if a.poster
                        .as_ref()
                        .is_some_and(|p| !self.files.contains_key(&p.id))
                    {
                        a.poster = None;
                    }
                }
                m
            })
            .collect();
        (messages, start as u64)
    }

    /// Remove an archived message (its line becomes [`DELETED`]). Returns it,
    /// so its files can be deleted.
    pub fn delete(&mut self, channel: &str, id: &str) -> Option<ChatMessage> {
        let path = self.path(channel);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .ok()?;
        let needle = format!("\"id\":{}", serde_json::to_string(id).ok()?);
        let mut reader = std::io::BufReader::new(&mut file);
        let mut line = Vec::new();
        let mut pos = 0u64;
        let found = loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break None,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&line);
                    if text.contains(&needle) {
                        if let Ok(m) = serde_json::from_slice::<ChatMessage>(&line) {
                            if m.id == id {
                                break Some((pos, line.len() - 1, m));
                            }
                        }
                    }
                    pos += n as u64;
                }
            }
        };
        let (at, len, message) = found?;
        drop(reader);
        let blank = format!("{DELETED:<len$}");
        let written = file
            .seek(SeekFrom::Start(at))
            .and_then(|_| file.write_all(blank.as_bytes()));
        if let Err(e) = written {
            error!(
                "chat",
                "Deleting an older message in #{channel} failed: {e}"
            );
            return None;
        }
        self.files.retain(|_, f| f.msg != message.id);
        Some(message)
    }

    /// A text channel was renamed.
    pub fn rename(&mut self, from: &str, to: &str) {
        let (old, new) = (self.path(from), self.path(to));
        if old.exists() {
            if let Err(e) = std::fs::rename(&old, &new) {
                error!("chat", "Moving {} failed: {e}", old.display());
            }
        }
        if let Some(offsets) = self.lines.remove(from) {
            self.lines.insert(to.to_string(), offsets);
        }
        for f in self.files.values_mut().filter(|f| f.channel == from) {
            f.channel = to.to_string();
        }
    }

    /// A text channel was deleted: move its file to `to` (the trash) and forget
    /// it. Returns its archived message count and files.
    pub fn take(&mut self, channel: &str, to: &Path) -> (usize, Vec<String>) {
        let from = self.path(channel);
        if from.exists() {
            if let Err(e) = std::fs::rename(&from, to) {
                error!("chat", "Moving {} to the trash failed: {e}", from.display());
            }
        }
        let count = self.lines.remove(channel).map_or(0, |o| o.len() - 1);
        let ids: Vec<String> = self
            .files
            .iter()
            .filter(|(_, f)| f.channel == channel)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.files.remove(id);
        }
        (count, ids)
    }

    /// Bring back a channel's messages moved away by [`Archive::take`].
    /// Returns how many there are.
    pub fn put_back(
        &mut self,
        channel: &str,
        from: &Path,
        attachments: &Path,
    ) -> std::io::Result<usize> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::rename(from, self.path(channel))?;
        Ok(self.index(channel, attachments).len())
    }

    /// The oldest archived message that still has files: (time, message id).
    pub fn oldest_with_files(&self) -> Option<(u64, String)> {
        self.files
            .values()
            .min_by_key(|f| f.ts)
            .map(|f| (f.ts, f.msg.clone()))
    }

    /// Forget a message's files (deleted for the storage limit). Returns their ids.
    pub fn expire(&mut self, msg: &str) -> Vec<String> {
        let ids: Vec<String> = self
            .files
            .iter()
            .filter(|(_, f)| f.msg == msg)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.files.remove(id);
        }
        ids
    }
}

/// A channel name as a file name: letters, digits, `-` and `_` as they are,
/// anything else as `%xx` bytes.
pub(crate) fn file_name(channel: &str) -> String {
    let mut out = String::new();
    for b in channel.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, ch: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            channel: ch.into(),
            author: "a".into(),
            author_id: "1".into(),
            text: format!("text {id}"),
            ts: 1,
            attachments: Vec::new(),
        }
    }

    fn channels() -> Vec<String> {
        vec!["general".into(), "a b".into()]
    }

    #[test]
    fn pages_deletes_and_reloads() {
        let dir = std::env::temp_dir().join(format!("tuffcord-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut a, _) = Archive::load(&dir, &channels());
        let all: Vec<ChatMessage> = (0..120).map(|i| msg(&format!("m{i}"), "general")).collect();
        a.append(&all);
        a.append(&[msg("x", "a b")]);
        let (page, start) = a.page("general", None);
        assert_eq!(page.len(), PAGE);
        assert_eq!(page[0].id, "m70");
        assert_eq!(start, 70);
        let (page, start) = a.page("general", Some(start));
        assert_eq!((page[0].id.as_str(), start), ("m20", 20));
        let (page, start) = a.page("general", Some(start));
        assert_eq!((page.len(), start), (20, 0));
        assert_eq!(a.page("general", Some(0)).0.len(), 0);

        assert_eq!(a.delete("general", "m5").unwrap().id, "m5");
        assert!(a.delete("general", "m5").is_none());
        let (page, _) = a.page("general", Some(20));
        assert_eq!(page.len(), 19);
        assert!(page.iter().all(|m| m.id != "m5"));

        // A line cut short by a crash is dropped on load.
        let path = a.path("general");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"id\":\"half")
            .unwrap();
        let (b, ids) = Archive::load(&dir, &channels());
        assert_eq!(ids.len(), 120); // 119 in general (one deleted) + 1 in "a b"
        assert_eq!(b.page("general", None).0.last().unwrap().id, "m119");
        assert_eq!(b.page("a b", None).0[0].id, "x");
        a = b;
        a.rename("a b", "c");
        assert_eq!(a.page("c", None).0[0].channel, "c");
        let away = dir.join("c-away.jsonl");
        assert_eq!(a.take("c", &away).0, 1);
        assert!(a.page("c", None).0.is_empty() && away.exists());
        assert_eq!(a.put_back("c", &away, &dir).unwrap(), 1);
        assert_eq!(a.page("c", None).0[0].id, "x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
