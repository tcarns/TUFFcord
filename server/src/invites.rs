//! One-time invites: the only way to create an account. Made only in the
//! server console (`invite`, or `tuffcord invite` on a Linux server); each
//! creates one account and works for 24 hours, up to 10 waiting at once.

use crate::accounts::{token_hash, Invite};
use crate::{config, now_ms, Shared, State};
use proto::ServerMsg;

pub const CONSOLE_HELP: &str =
    "  invite             make a one-time invite code (creates one account, 24 hours)";

const LIFETIME_MS: u64 = 24 * 60 * 60 * 1000;
/// `Invite::by` for invites made in the server console (account ids start at 1).
pub const CONSOLE: u32 = 0;
/// Unused invites at most.
const MAX_OPEN: usize = 10;

fn prune(st: &mut State) {
    let now = now_ms();
    let list = st.accounts.invites();
    let before = list.len();
    // Admins made invites in the app before 0.12; only the console's count now.
    list.retain(|i| i.expires > now && i.by == CONSOLE);
    if list.len() != before {
        st.accounts.dirty = true;
    }
}

/// A new code, with when it expires.
fn make(st: &mut State) -> Result<String, String> {
    prune(st);
    if st.accounts.invites().len() >= MAX_OPEN {
        return Err(format!("There are already {MAX_OPEN} unused invites. Wait for some to be used or to expire (24 hours after they were made)."));
    }
    let code = config::random_password();
    let expires = now_ms() + LIFETIME_MS;
    st.accounts.invites().push(Invite {
        hash: token_hash(&code),
        by: CONSOLE,
        expires,
    });
    Ok(code)
}

/// An app from before 0.12 asked for an invite.
pub fn create(st: &State, conn: u32) {
    st.send(
        conn,
        &ServerMsg::Error {
            code: "invite_failed".into(),
            message: "Invites are made in the server window now: type \"invite\" there.".into(),
        },
    );
}

/// `invite`, `invites` and `uninvite` typed in the server console.
pub fn console(shared: &Shared, cmd: &str, arg: &str) -> Option<String> {
    let mut st = shared.lock().unwrap();
    match cmd {
        "invite" => Some(invite(&mut st)),
        "invites" => Some(list(&mut st)),
        "uninvite" => Some(uninvite(&mut st, arg)),
        _ => None,
    }
}

fn invite(st: &mut State) -> String {
    let code = match make(st) {
        Ok(x) => x,
        Err(e) => return e,
    };
    admin_log!(st, "The server console made an invite");
    if let Err(e) = st.accounts.save() {
        error!("auth", "{e}");
    }
    format!("Invite code: {code}\nIt creates one account and works for 24 hours. In the app: Create account, then paste it into \"Invite code\".")
}

/// Oldest first, so the numbers stay put as new ones are made.
fn sorted(st: &mut State) -> Vec<Invite> {
    prune(st);
    let mut list = st.accounts.invites().clone();
    list.sort_by_key(|i| i.expires);
    list
}

fn list(st: &mut State) -> String {
    let list = sorted(st);
    if list.is_empty() {
        return "No unused invites. Type \"invite\" to make one.".into();
    }
    let now = now_ms();
    let lines: Vec<String> = list
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let made = chrono::DateTime::from_timestamp_millis((i.expires - LIFETIME_MS) as i64)
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%b %-d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let mins = (i.expires.saturating_sub(now) / 60_000).max(1);
            let left = if mins >= 60 {
                format!("{} h {} min", mins / 60, mins % 60)
            } else {
                format!("{mins} min")
            };
            format!("  {}. made {made}, {left} left", n + 1)
        })
        .collect();
    format!(
        "{} unused (codes aren't kept, so they can't be shown again):\n{}\nCancel one with \"uninvite <n>\", or all with \"uninvite all\".",
        list.len(),
        lines.join("\n")
    )
}

fn uninvite(st: &mut State, arg: &str) -> String {
    let list = sorted(st);
    let gone: Vec<String> = if arg.eq_ignore_ascii_case("all") {
        list.iter().map(|i| i.hash.clone()).collect()
    } else {
        match arg.parse::<usize>() {
            Ok(n) if (1..=list.len()).contains(&n) => vec![list[n - 1].hash.clone()],
            _ if list.is_empty() => return "No unused invites to cancel.".into(),
            _ => return format!(
                "Type \"uninvite <n>\" with n from 1 to {} (see \"invites\"), or \"uninvite all\".",
                list.len()
            ),
        }
    };
    if gone.is_empty() {
        return "No unused invites to cancel.".into();
    }
    st.accounts.invites().retain(|i| !gone.contains(&i.hash));
    let what = if gone.len() == 1 {
        "an invite".to_string()
    } else {
        format!("{} invites", gone.len())
    };
    admin_log!(st, "The server console cancelled {what}");
    if let Err(e) = st.accounts.save() {
        error!("auth", "{e}");
    }
    format!("Cancelled {what}. {} left.", list.len() - gone.len())
}

/// Is `code` an unused invite? (Doesn't use it up; see `redeem`.)
pub fn valid(st: &mut State, code: &str) -> bool {
    prune(st);
    let h = token_hash(code.trim());
    st.accounts.invites().iter().any(|i| i.hash == h)
}

/// Use up the invite `code`. False if it's gone.
pub fn redeem(st: &mut State, code: &str) -> bool {
    prune(st);
    let h = token_hash(code.trim());
    let list = st.accounts.invites();
    let Some(i) = list.iter().position(|i| i.hash == h) else {
        return false;
    };
    list.remove(i);
    st.accounts.dirty = true;
    true
}
