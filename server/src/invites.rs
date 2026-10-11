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

/// `invite` typed in the server console.
pub fn console(shared: &Shared, cmd: &str) -> Option<String> {
    if cmd != "invite" {
        return None;
    }
    let mut st = shared.lock().unwrap();
    let code = match make(&mut st) {
        Ok(x) => x,
        Err(e) => return Some(e),
    };
    admin_log!(&mut st, "The server console made an invite");
    if let Err(e) = st.accounts.save() {
        error!("auth", "{e}");
    }
    Some(format!("Invite code: {code}\nIt creates one account and works for 24 hours. In the app: Create account, then paste it into \"Invite code\"."))
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
