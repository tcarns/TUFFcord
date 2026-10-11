//! One-time invites: an admin makes a code that creates one account in place
//! of the group password, for a week. They work even with `allow_signup` off,
//! which then only stops the group password creating accounts.

use crate::accounts::{token_hash, Invite};
use crate::{config, now_ms, State};
use proto::ServerMsg;

const LIFETIME_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// Unused invites at most, so a stolen admin account can't hand out hundreds.
const MAX_OPEN: usize = 20;

fn prune(st: &mut State) {
    let now = now_ms();
    let list = st.accounts.invites();
    let before = list.len();
    list.retain(|i| i.expires > now);
    if list.len() != before {
        st.accounts.dirty = true;
    }
}

/// An admin asked for an invite.
pub fn create(st: &mut State, conn: u32) {
    let fail = |st: &State, message: &str| {
        st.send(
            conn,
            &ServerMsg::Error {
                code: "invite_failed".into(),
                message: message.into(),
            },
        )
    };
    let Some(by) = st.clients.get(&conn).and_then(|c| c.account) else {
        return;
    };
    if !st.accounts.get(by).is_some_and(|a| a.admin) {
        return fail(st, "Only admins can make invites.");
    }
    prune(st);
    if st.accounts.invites().len() >= MAX_OPEN {
        return fail(
            st,
            &format!("There are already {MAX_OPEN} unused invites. Wait for some to be used or to expire (after a week)."),
        );
    }
    let code = config::random_password();
    let expires = now_ms() + LIFETIME_MS;
    st.accounts.invites().push(Invite {
        hash: token_hash(&code),
        by,
        expires,
    });
    let who = st.who(conn);
    admin_log!(st, "{who} made an invite");
    if let Err(e) = st.accounts.save() {
        error!("auth", "{e}");
    }
    st.send(conn, &ServerMsg::Invite { code, expires });
}

/// Is `code` an unused invite? (Doesn't use it up; see `redeem`.)
pub fn valid(st: &mut State, code: &str) -> bool {
    prune(st);
    let h = token_hash(code.trim());
    st.accounts.invites().iter().any(|i| i.hash == h)
}

/// Use up the invite `code`. Returns who made it, or None if it's gone.
pub fn redeem(st: &mut State, code: &str) -> Option<u32> {
    prune(st);
    let h = token_hash(code.trim());
    let list = st.accounts.invites();
    let i = list.iter().position(|i| i.hash == h)?;
    let by = list.remove(i).by;
    st.accounts.dirty = true;
    Some(by)
}
