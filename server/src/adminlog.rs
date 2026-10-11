//! What admins did (kicks, bans, resets, channel and message deletes), kept
//! in accounts.json so admins can look back in the app, and logged as warnings.

use crate::{now_ms, State};
use proto::{AdminLogEntry, ServerMsg};

/// Entries kept (oldest dropped).
const KEEP: usize = 200;
/// Entries sent to the app at once.
const SEND: usize = 100;

/// `admin_log!(st, "{by} kicked {name}")`: log it and keep it for the admin log.
macro_rules! admin_log {
    ($st:expr, $($arg:tt)*) => {{
        let text = format!($($arg)*);
        $crate::adminlog::record($st, text);
    }};
}

pub fn record(st: &mut State, text: String) {
    warn!("admin", "{text}");
    let log = st.accounts.admin_log();
    log.push(crate::accounts::LogEntry { ts: now_ms(), text });
    if log.len() > KEEP {
        let extra = log.len() - KEEP;
        log.drain(..extra);
    }
    st.accounts.dirty = true;
}

/// An admin opened the admin log.
pub fn send(st: &mut State, conn: u32) {
    let admin = st
        .clients
        .get(&conn)
        .and_then(|c| c.account)
        .and_then(|a| st.accounts.get(a))
        .is_some_and(|a| a.admin);
    if !admin {
        return;
    }
    let entries = st
        .accounts
        .admin_log()
        .iter()
        .rev()
        .take(SEND)
        .map(|e| AdminLogEntry {
            ts: e.ts,
            text: e.text.clone(),
        })
        .collect();
    st.send(conn, &ServerMsg::AdminLog { entries });
}
