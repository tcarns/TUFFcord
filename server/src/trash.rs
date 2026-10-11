//! Deleted text channels: their messages and files wait in `data/trash` for
//! 30 days, so a mis-click or a stolen admin account can be undone from the
//! server window (`trash`, `restore <n>`).

use crate::{attachments_dir, channels, history, now_ms, Shared, State};
use proto::ChatMessage;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const KEEP_MS: u64 = 30 * 24 * 60 * 60 * 1000;

pub const CONSOLE_HELP: &str = "\
  trash              list deleted text channels (kept 30 days)
  restore <n>        bring back deleted channel number n from that list";

fn trash_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("trash")
}

/// Move a deleted text channel's messages (`recent` from memory, the rest
/// from its history file) and files into the trash. Returns (messages, files).
pub fn keep(st: &mut State, channel: &str, recent: Vec<ChatMessage>) -> (usize, usize) {
    let to =
        trash_dir(&st.cfg.data_dir).join(format!("{}-{}", now_ms(), history::file_name(channel)));
    let files_to = to.join("files");
    if let Err(e) = std::fs::create_dir_all(&files_to)
        .and_then(|_| std::fs::write(to.join("channel.txt"), channel))
    {
        error!("admin", "Couldn't make {} ({e})", to.display());
    }
    // Every message in one file first, then move that.
    st.archive.append(&recent);
    let (count, ids) = st.archive.take(channel, &to.join("messages.jsonl"));
    let attachments = attachments_dir(&st.cfg);
    let mut moved = 0;
    for id in ids {
        let from = attachments.join(&id);
        let Ok(meta) = std::fs::metadata(&from) else {
            continue;
        };
        match std::fs::rename(&from, files_to.join(&id)) {
            Ok(()) => {
                st.files.stored = st.files.stored.saturating_sub(meta.len());
                moved += 1;
            }
            Err(e) => error!(
                "admin",
                "Moving {} to the trash failed ({e})",
                from.display()
            ),
        }
    }
    (count, moved)
}

struct Entry {
    path: PathBuf,
    channel: String,
    deleted: u64,
}

/// What's in the trash, newest first.
fn entries(data_dir: &Path) -> Vec<Entry> {
    let mut list: Vec<Entry> = std::fs::read_dir(trash_dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let deleted = name.split('-').next()?.parse().ok()?;
            let channel = std::fs::read_to_string(path.join("channel.txt")).ok()?;
            Some(Entry {
                path,
                channel: channel.trim().to_string(),
                deleted,
            })
        })
        .collect();
    list.sort_by_key(|e| std::cmp::Reverse(e.deleted));
    list
}

/// Delete what has been in the trash for over 30 days.
pub fn purge(data_dir: &Path) {
    let now = now_ms();
    for e in entries(data_dir) {
        if now.saturating_sub(e.deleted) > KEEP_MS {
            match std::fs::remove_dir_all(&e.path) {
                Ok(()) => info!(
                    "admin",
                    "Emptied #{} from the trash (deleted over 30 days ago)", e.channel
                ),
                Err(err) => error!(
                    "admin",
                    "Couldn't empty {} from the trash ({err})",
                    e.path.display()
                ),
            }
        }
    }
}

/// `trash` and `restore <n>` typed in the server window. None if `cmd` isn't one.
pub fn console(shared: &Shared, cmd: &str, arg: &str) -> Option<String> {
    let mut st = shared.lock().unwrap();
    let list = entries(&st.cfg.data_dir);
    match cmd {
        "trash" if list.is_empty() => Some("The trash is empty.".into()),
        "trash" => Some(
            list.iter()
                .enumerate()
                .map(|(i, e)| {
                    format!(
                        "  {}. #{} (deleted {})",
                        i + 1,
                        e.channel,
                        crate::log::local_time(e.deleted)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\nType \"restore <number>\" to bring one back.",
        ),
        "restore" => {
            let Some(e) = arg
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| list.get(i))
            else {
                return Some("Type \"trash\" for the list, then \"restore <number>\".".into());
            };
            Some(restore(&mut st, e).unwrap_or_else(|err| err))
        }
        _ => None,
    }
}

fn restore(st: &mut State, e: &Entry) -> Result<String, String> {
    let name = &e.channel;
    if st
        .cfg
        .text_channels
        .iter()
        .any(|c| c.to_lowercase() == name.to_lowercase())
    {
        return Err(format!(
            "There's already a text channel called #{name}. Rename or delete it first."
        ));
    }
    let attachments = attachments_dir(&st.cfg);
    let _ = std::fs::create_dir_all(&attachments);
    let mut files = 0;
    for f in std::fs::read_dir(e.path.join("files"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        if std::fs::rename(f.path(), attachments.join(f.file_name())).is_ok() {
            st.files.stored += size;
            files += 1;
        }
    }
    let messages_file = e.path.join("messages.jsonl");
    let count = if messages_file.exists() {
        st.archive
            .put_back(name, &messages_file, &attachments)
            .map_err(|err| format!("Couldn't restore #{name}'s messages ({err})."))?
    } else {
        0
    };
    Arc::make_mut(&mut st.cfg).text_channels.push(name.clone());
    st.history.entry(name.clone()).or_default();
    channels::saved_and_sent(st);
    let _ = std::fs::remove_dir_all(&e.path);
    admin_log!(
        st,
        "The server window restored the text channel #{name} ({count} message(s), {files} file(s))"
    );
    Ok(format!(
        "Restored #{name}: {count} message(s), {files} file(s)."
    ))
}
