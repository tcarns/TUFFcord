//! Your signed-in devices: listing them, signing one (or all others) out, and
//! the "new sign-in from ..." notice when your account signs in from an
//! address it hasn't used before.

use crate::{now_ms, State};
use proto::{ServerMsg, SessionInfo};

/// Characters of a sign-in's hash used as its id in the app.
const ID_LEN: usize = 12;
/// Notices kept for later at most.
const MAX_NOTICES: usize = 5;

fn account_of(st: &State, conn: u32) -> Option<(u32, Option<String>)> {
    let c = st.clients.get(&conn)?;
    Some((c.account?, c.token_hash.clone()))
}

pub fn list(st: &State, conn: u32) {
    let Some((account, current)) = account_of(st, conn) else {
        return;
    };
    let Some(a) = st.accounts.get(account) else {
        return;
    };
    let mut list: Vec<SessionInfo> = a
        .tokens
        .iter()
        .map(|t| SessionInfo {
            id: t.hash[..ID_LEN.min(t.hash.len())].to_string(),
            created: t.created,
            last_used: t.last_used,
            ip: t.ip.clone(),
            current: current.as_deref() == Some(t.hash.as_str()),
        })
        .collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.last_used));
    st.send(conn, &ServerMsg::Sessions { list });
}

/// Sign out the device `id`, or every device but this one when None.
pub fn end(st: &mut State, conn: u32, id: Option<String>) {
    let Some((account, current)) = account_of(st, conn) else {
        return;
    };
    let gone: Vec<String> = match st.accounts.get(account) {
        Some(a) => a
            .tokens
            .iter()
            .filter(|t| match &id {
                Some(id) => id.len() >= ID_LEN && t.hash.starts_with(id.as_str()),
                None => current.as_deref() != Some(t.hash.as_str()),
            })
            .map(|t| t.hash.clone())
            .collect(),
        None => return,
    };
    for h in &gone {
        st.accounts.revoke_token(h);
    }
    if let Err(e) = st.accounts.save() {
        error!("auth", "{e}");
    }
    // Their connections go too (with no saved sign-in, an older app's included
    // when it's "everywhere else").
    let conns: Vec<u32> = st
        .clients
        .values()
        .filter(|c| c.authed && c.account == Some(account) && c.id != conn)
        .filter(|c| match &c.token_hash {
            Some(h) => gone.contains(h),
            None => id.is_none(),
        })
        .map(|c| c.id)
        .collect();
    for c in &conns {
        if st.clients.get(c).is_some_and(|c| c.voice.is_some()) {
            crate::leave_voice(st, *c, Some("signed out"));
        }
        st.refuse(
            *c,
            "session_expired",
            "You were signed out from another device.",
            false,
        );
    }
    info!(
        "auth",
        "{} signed out {} other device(s)",
        st.who(conn),
        gone.len().max(conns.len())
    );
    let text = match (&id, gone.len().max(conns.len())) {
        (_, 0) => "That device was already signed out.".to_string(),
        (Some(_), _) => "Signed that device out.".to_string(),
        (None, n) => format!(
            "Signed out {n} other device{}.",
            if n == 1 { "" } else { "s" }
        ),
    };
    st.send(conn, &ServerMsg::Notice { text });
    list(st, conn);
}

/// Before a password sign-in from `ip` is recorded: if the account has signed
/// in before but never from there, tell its other devices (now, or the next
/// time one connects).
pub fn note_new_address(st: &mut State, account: u32, ip: &str) {
    let Some(a) = st.accounts.get(account) else {
        return;
    };
    let known = !a.last_ip.is_empty() || !a.tokens.is_empty();
    if !known || a.last_ip == ip || a.tokens.iter().any(|t| t.ip == ip) {
        return;
    }
    let text = format!(
        "Your account signed in from a new address ({ip}) at {}. If that wasn't you, change your password in Settings > Account: that signs out every other device.",
        crate::log::local_time(now_ms())
    );
    let online: Vec<u32> = st
        .clients
        .values()
        .filter(|c| c.authed && c.account == Some(account))
        .map(|c| c.id)
        .collect();
    info!("auth", "{} signed in from a new address ({ip})", a.name);
    if online.is_empty() {
        let a = st.accounts.get_mut(account).unwrap();
        a.notices.push(text);
        if a.notices.len() > MAX_NOTICES {
            a.notices.remove(0);
        }
        st.accounts.dirty = true;
    } else {
        for c in online {
            st.send(c, &ServerMsg::Notice { text: text.clone() });
        }
    }
}

/// A device signed in with its saved sign-in: show it what it missed.
pub fn deliver_notices(st: &mut State, conn: u32, account: u32) {
    let Some(a) = st.accounts.get_mut(account) else {
        return;
    };
    if a.notices.is_empty() {
        return;
    }
    let list = std::mem::take(&mut a.notices);
    st.accounts.dirty = true;
    for text in list {
        st.send(conn, &ServerMsg::Notice { text });
    }
}
