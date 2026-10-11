//! What admins do to accounts (kick, ban, reset a password, make an admin,
//! delete), from the app or the server window, with the owner rule.

use crate::{accounts, config, limits, Shared, State};
use proto::{Account, AdminAction, ServerMsg};

/// What an admin (or the server window) can do. `by` is who's doing it, for messages.
pub enum Act {
    Kick,
    Ban,
    Unban,
    /// With the new temporary password's hash (hashed off the voice thread).
    ResetPassword {
        temp_hash: String,
    },
    SetAdmin(bool),
    Delete,
}

/// Do `act` to `target`. `actor` is the admin's account (None = the server window).
pub fn act(
    st: &mut State,
    actor: Option<u32>,
    by: &str,
    target: u32,
    act: Act,
) -> Result<String, String> {
    let Some(t) = st.accounts.get(target) else {
        return Err("That account doesn't exist any more.".into());
    };
    let name = t.name.clone();
    let (t_admin, t_banned) = (t.admin, t.banned);
    let is_self = actor == Some(target);
    // Admins can't act on each other, so one stolen admin password can't take
    // the server over: only the owner (and the server window) can.
    let by_owner = actor.is_none() || actor == st.accounts.owner();
    if !by_owner && !is_self {
        if matches!(act, Act::SetAdmin(_)) {
            return Err("Only the server's owner can give or take admin.".into());
        }
        if t_admin && matches!(act, Act::Kick | Act::ResetPassword { .. } | Act::Delete) {
            return Err(format!(
                "{name} is an admin. Only the server's owner can do that."
            ));
        }
    }
    let result = match act {
        Act::Kick => {
            if is_self {
                return Err("You can't kick yourself.".into());
            }
            let n = st.disconnect_account(
                target,
                "kicked",
                &format!("{by} removed you from the server. You can sign back in."),
                None,
            );
            if n == 0 {
                return Err(format!("{name} isn't online."));
            }
            admin_log!(st, "{by} kicked {name}");
            format!("Kicked {name}.")
        }
        Act::Ban => {
            if is_self {
                return Err("You can't ban yourself.".into());
            }
            if t_admin {
                return Err(format!("{name} is an admin. Remove their admin first."));
            }
            if t_banned {
                return Err(format!("{name} is already banned."));
            }
            let a = st.accounts.get_mut(target).unwrap();
            a.banned = true;
            a.tokens.clear();
            st.save_accounts();
            st.disconnect_account(
                target,
                "banned",
                "You've been banned from this server.",
                None,
            );
            admin_log!(st, "{by} banned {name}");
            format!("Banned {name}. They can't sign in or create another account from the same address.")
        }
        Act::Unban => {
            if !t_banned {
                return Err(format!("{name} isn't banned."));
            }
            st.accounts.get_mut(target).unwrap().banned = false;
            st.save_accounts();
            admin_log!(st, "{by} unbanned {name}");
            format!("{name} can sign in again.")
        }
        Act::ResetPassword { temp_hash } => {
            if is_self {
                return Err("Change your own password in Settings instead.".into());
            }
            let a = st.accounts.get_mut(target).unwrap();
            a.password_hash = temp_hash;
            a.must_change_password = true;
            a.tokens.clear();
            st.save_accounts();
            st.disconnect_account(
                target,
                "password_reset",
                &format!("{by} reset your password. Ask them for your temporary password."),
                None,
            );
            admin_log!(st, "{by} reset {name}'s password");
            format!("{name}'s temporary password is below. Send it to them; they'll pick a new one when they sign in.")
        }
        Act::SetAdmin(admin) => {
            if t_admin == admin {
                return Err(format!(
                    "{name} {} an admin.",
                    if admin { "is already" } else { "isn't" }
                ));
            }
            if !admin && st.accounts.admin_count() == 1 {
                return Err("There has to be at least one admin.".into());
            }
            let a = st.accounts.get_mut(target).unwrap();
            a.admin = admin;
            if !admin {
                a.owner = false;
            }
            st.accounts.ensure_owner();
            st.save_accounts();
            let me = Account {
                id: target,
                name: name.clone(),
                admin,
            };
            let conns: Vec<u32> = st
                .clients
                .values()
                .filter(|c| c.account == Some(target))
                .map(|c| c.id)
                .collect();
            for id in conns {
                st.clients.get_mut(&id).unwrap().admin = admin;
                st.send(
                    id,
                    &ServerMsg::AccountUpdated {
                        account: me.clone(),
                        token: None,
                    },
                );
            }
            st.broadcast_presence();
            admin_log!(
                st,
                "{by} {} {name}",
                if admin {
                    "made an admin of"
                } else {
                    "removed admin from"
                }
            );
            if admin {
                format!("{name} is now an admin.")
            } else {
                format!("{name} is no longer an admin.")
            }
        }
        Act::Delete => {
            if is_self {
                return Err("You can't delete your own account.".into());
            }
            if t_admin && st.accounts.admin_count() == 1 {
                return Err("That's the only admin. Make someone else an admin first.".into());
            }
            st.disconnect_account(target, "session_expired", "Your account was deleted.", None);
            st.accounts.delete(target);
            let cfg = st.cfg.clone();
            st.avatars.remove(&cfg, target);
            st.accounts.ensure_owner();
            st.save_accounts();
            admin_log!(st, "{by} deleted {name}'s account");
            format!("Deleted {name}'s account. The name is free again.")
        }
    };
    st.send_accounts_to_admins();
    st.broadcast_members();
    Ok(result)
}

/// An admin in the app asked for something.
pub async fn admin(shared: &Shared, conn: u32, action: AdminAction) {
    let (actor, by) = {
        let st = shared.lock().unwrap();
        let Some(actor) = st.account_of(conn) else {
            return;
        };
        // Check the account, not the connection: admin may have just been removed.
        if !st.accounts.get(actor).is_some_and(|a| a.admin) {
            st.send(
                conn,
                &ServerMsg::Error {
                    code: "not_admin".into(),
                    message: "Only admins can do that.".into(),
                },
            );
            return;
        }
        (actor, st.who(conn))
    };
    let mut secret = None;
    let (target, act_) = match action {
        AdminAction::Kick { account } => (account, Act::Kick),
        AdminAction::Ban { account } => (account, Act::Ban),
        AdminAction::Unban { account } => (account, Act::Unban),
        AdminAction::SetAdmin { account, admin } => (account, Act::SetAdmin(admin)),
        AdminAction::ResetPassword { account } => {
            let temp = config::random_password();
            let Some(hash) = limits::hash(temp.clone()).await else {
                return;
            };
            secret = Some(temp);
            (account, Act::ResetPassword { temp_hash: hash })
        }
    };
    let mut st = shared.lock().unwrap();
    match act(&mut st, Some(actor), &by, target, act_) {
        Ok(text) => st.send(conn, &ServerMsg::AdminResult { text, secret }),
        Err(message) => st.send(
            conn,
            &ServerMsg::Error {
                code: "admin_failed".into(),
                message,
            },
        ),
    }
}

// ---------------------------------------------------------------- server window

pub const CONSOLE_HELP: &str = "\
  users              list accounts
  admin <name>       make someone an admin (unadmin <name> to undo)
  owner <name>       make someone the owner: the admin other admins can't act on
  kick <name>        disconnect them (they can sign back in)
  ban <name>         disconnect them and stop them signing in (unban <name> to undo)
  resetpw <name>     give them a temporary password
  deluser <name>     delete an account, freeing the name";

/// Account commands typed in the server window. Returns None if `cmd` isn't one.
pub fn console(shared: &Shared, cmd: &str, arg: &str) -> Option<String> {
    let needs_name = matches!(
        cmd,
        "admin" | "unadmin" | "owner" | "kick" | "ban" | "unban" | "resetpw" | "deluser"
    );
    if cmd == "users" {
        let st = shared.lock().unwrap();
        let list = st.summaries();
        if list.is_empty() {
            return Some(
                "No accounts yet. People create one in the app with the group password.".into(),
            );
        }
        let mut out = format!(
            "{} account{}:",
            list.len(),
            if list.len() == 1 { "" } else { "s" }
        );
        for a in list {
            let mut tags = Vec::new();
            if a.online {
                tags.push("online".to_string());
            } else if a.last_seen > 0 {
                tags.push(format!("last seen {}", crate::log::local_time(a.last_seen)));
            }
            if a.admin {
                tags.push("admin".into());
            }
            if a.banned {
                tags.push("BANNED".into());
            }
            out.push_str(&format!("\n  {}  ({})", a.name, tags.join(", ")));
        }
        return Some(out);
    }
    if !needs_name {
        return None;
    }
    if arg.is_empty() {
        return Some(format!("Type: {cmd} <name>"));
    }
    let target = {
        let st = shared.lock().unwrap();
        match st.accounts.by_name(arg) {
            Some(a) => a.id,
            None => {
                return Some(format!(
                    "No account called \"{arg}\". Type \"users\" for the list."
                ))
            }
        }
    };
    if cmd == "owner" {
        let mut st = shared.lock().unwrap();
        if !st.accounts.get(target).is_some_and(|a| a.admin) {
            let _ = act(
                &mut st,
                None,
                "The server admin",
                target,
                Act::SetAdmin(true),
            );
        }
        st.accounts.set_owner(target);
        st.save_accounts();
        let name = st
            .accounts
            .get(target)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        admin_log!(&mut st, "The server admin made {name} the owner");
        st.send_accounts_to_admins();
        return Some(format!("{name} is now the owner (and an admin)."));
    }
    let mut secret = None;
    let act_ = match cmd {
        "admin" => Act::SetAdmin(true),
        "unadmin" => Act::SetAdmin(false),
        "kick" => Act::Kick,
        "ban" => Act::Ban,
        "unban" => Act::Unban,
        "deluser" => Act::Delete,
        _ => {
            // resetpw: hash here, on the console thread, with the state unlocked.
            let temp = config::random_password();
            let hash = accounts::hash_password(&temp);
            secret = Some(temp);
            Act::ResetPassword { temp_hash: hash }
        }
    };
    let mut st = shared.lock().unwrap();
    Some(match act(&mut st, None, "The server admin", target, act_) {
        Ok(text) => match secret {
            Some(s) => format!("{text}\n  Temporary password: {s}"),
            None => text,
        },
        Err(e) => e,
    })
}
