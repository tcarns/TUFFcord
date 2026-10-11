//! Signing in, creating accounts, and account management (by the person
//! themselves, by admins in the app, and from the server window).
//!
//! Password hashing is slow on purpose, so it runs on tokio's blocking pool
//! with the state unlocked; voice keeps flowing for everyone meanwhile.

use crate::accounts::{self, Store};
use crate::{config, leave_voice, limits, now_ms, Shared, State};
use proto::{Account, AccountSummary, AdminAction, Auth, Member, ServerMsg, FEATURE_ACCOUNTS};
use proto::{
    FEATURE_ATTACHMENTS, FEATURE_CHANNELS, FEATURE_CHANNEL_EDIT, FEATURE_FILES,
    FEATURE_OLDER_MESSAGES, PROTOCOL_VERSION,
};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------- helpers on State

impl State {
    fn account_of(&self, conn: u32) -> Option<u32> {
        self.clients.get(&conn).and_then(|c| c.account)
    }

    fn summaries(&self) -> Vec<AccountSummary> {
        let mut list: Vec<AccountSummary> = self
            .accounts
            .all()
            .iter()
            .map(|a| AccountSummary {
                id: a.id,
                name: a.name.clone(),
                admin: a.admin,
                banned: a.banned,
                online: self
                    .clients
                    .values()
                    .any(|c| c.authed && c.account == Some(a.id)),
                last_seen: a.last_seen,
            })
            .collect();
        list.sort_by_key(|a| a.name.to_lowercase());
        list
    }

    /// Everyone with an account (not banned), for the member list.
    pub fn members(&self) -> Vec<Member> {
        let mut list: Vec<Member> = self
            .accounts
            .all()
            .iter()
            .filter(|a| !a.banned)
            .map(|a| Member {
                account: a.id,
                name: a.name.clone(),
                admin: a.admin,
            })
            .collect();
        list.sort_by_key(|m| m.name.to_lowercase());
        list
    }

    /// The member list changed: tell everyone signed in.
    pub fn broadcast_members(&self) {
        self.broadcast(&ServerMsg::Members {
            list: self.members(),
        });
    }

    /// Admins see everyone's accounts; refresh their list after any change.
    pub fn send_accounts_to_admins(&self) {
        let admins: Vec<u32> = self
            .clients
            .values()
            .filter(|c| c.authed && c.admin)
            .map(|c| c.id)
            .collect();
        if admins.is_empty() {
            return;
        }
        let msg = ServerMsg::Accounts {
            list: self.summaries(),
        };
        for id in admins {
            self.send(id, &msg);
        }
    }

    /// Send an error and close the connection (after a pause, if `slow`, so
    /// password guessing is slow).
    fn refuse(&self, conn: u32, code: &str, message: &str, slow: bool) {
        let Some(c) = self.clients.get(&conn) else {
            return;
        };
        let tx = c.tx.clone();
        let text = serde_json::to_string(&ServerMsg::Error {
            code: code.into(),
            message: message.into(),
        })
        .unwrap();
        if slow {
            // Only from connection tasks (inside the async runtime).
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                let _ = tx.send(Message::Text(text.into())).await;
                let _ = tx.send(Message::Close(None)).await;
            });
        } else {
            // Also used from the server window's thread, so no awaiting here.
            let _ = tx.try_send(Message::Text(text.into()));
            let _ = tx.try_send(Message::Close(None));
        }
    }

    /// Disconnect every device signed in to `account`, telling them why.
    /// Returns how many were disconnected.
    pub fn disconnect_account(
        &mut self,
        account: u32,
        code: &str,
        message: &str,
        except: Option<u32>,
    ) -> usize {
        let conns: Vec<u32> = self
            .clients
            .values()
            .filter(|c| c.authed && c.account == Some(account) && Some(c.id) != except)
            .map(|c| c.id)
            .collect();
        for &id in &conns {
            if self.clients.get(&id).is_some_and(|c| c.voice.is_some()) {
                leave_voice(self, id, Some("removed"));
            }
            self.refuse(id, code, message, false);
        }
        conns.len()
    }

    fn save_accounts(&mut self) {
        if let Err(e) = self.accounts.save() {
            error!("auth", "{e}");
        }
    }

    /// Mark connection `conn` as signed in to `account` and welcome it.
    fn sign_in(
        &mut self,
        conn: u32,
        account: u32,
        token_hash: Option<String>,
        token: Option<String>,
        how: &str,
    ) {
        let now = now_ms();
        let Some(ip) = self.clients.get(&conn).map(|c| c.ip.clone()) else {
            return; // they left while we were checking the password
        };
        let Some(a) = self.accounts.get_mut(account) else {
            return;
        };
        a.last_seen = now;
        a.last_ip = ip.clone();
        let me = Account {
            id: a.id,
            name: a.name.clone(),
            admin: a.admin,
        };
        let must_change = a.must_change_password;
        self.accounts.dirty = true;
        let file_key = crate::random_id() + &crate::random_id();
        {
            let c = self.clients.get_mut(&conn).unwrap();
            c.authed = true;
            c.name = me.name.clone();
            c.account = Some(me.id);
            c.admin = me.admin;
            c.token_hash = token_hash;
            c.signed_in_at = Instant::now();
            c.file_key = Some(file_key.clone());
        }
        self.files.keys.insert(file_key.clone(), conn);
        info!(
            "conn",
            "{} signed in from {ip}{how} ({} online)",
            me.name,
            self.online_count()
        );
        let welcome = ServerMsg::Welcome {
            id: conn,
            name: me.name.clone(),
            app_name: self.cfg.app_name.clone(),
            text_channels: self.cfg.text_channels.clone(),
            voice_channels: self.cfg.voice_channels.clone(),
            max_per_voice_channel: self.cfg.max_per_voice_channel,
            history: self.history.clone(),
            voice_state: self.voice_state(),
            users: self.users(),
            features: vec![
                FEATURE_ATTACHMENTS.to_string(),
                FEATURE_ACCOUNTS.to_string(),
                FEATURE_FILES.to_string(),
                FEATURE_CHANNELS.to_string(),
                FEATURE_CHANNEL_EDIT.to_string(),
                FEATURE_OLDER_MESSAGES.to_string(),
            ],
            max_attachment_bytes: self.cfg.max_attachment_bytes,
            account: Some(me),
            token,
            must_change_password: must_change,
            file_key: Some(file_key),
            members: self.members(),
        };
        self.send(conn, &welcome);
        self.broadcast_presence();
        self.send_accounts_to_admins();
        crate::storage::send_to_admins(self, Some(conn));
    }
}

/// Check a password with the wrong-password limits: the attempt counts as
/// wrong until the (slow) check says otherwise, so guesses sent all at once
/// can't slip past the limit.
async fn checked_verify(
    shared: &Shared,
    ip: &str,
    name: &str,
    pw: String,
    hash: Option<String>,
) -> bool {
    shared.lock().unwrap().failures.record(ip, Some(name));
    let ok = limits::verify(pw, hash).await;
    if ok {
        shared.lock().unwrap().failures.forgive(ip, Some(name));
    }
    ok
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

// ---------------------------------------------------------------- signing in

pub async fn hello(
    shared: &Shared,
    conn: u32,
    name: String,
    password: String,
    version: u32,
    accounts_aware: bool,
    auth: Option<Auth>,
) {
    let ip = {
        let mut st = shared.lock().unwrap();
        let Some(c) = st.clients.get(&conn) else {
            return;
        };
        if c.authed {
            return;
        }
        let ip = c.ip.clone();
        if version != PROTOCOL_VERSION {
            warn!(
                "auth",
                "{ip} has a different app version (protocol {version}, server {PROTOCOL_VERSION})"
            );
            st.refuse(
                conn,
                "bad_version",
                "Your TUFFcord app is a different version than the server. Get the latest one.",
                false,
            );
            return;
        }
        if st.failures.ip_blocked(&ip) {
            warn!(
                "auth",
                "Refused a sign-in from {ip}: too many wrong passwords recently"
            );
            st.refuse(
                conn,
                "too_many_attempts",
                "Too many wrong passwords. Wait a few minutes and try again.",
                true,
            );
            return;
        }
        ip
    };
    match auth {
        Some(Auth::Token { token }) => {
            let hash = accounts::token_hash(&token);
            let mut st = shared.lock().unwrap();
            match st.accounts.use_token(&hash, now_ms()) {
                Some(id) if st.accounts.get(id).is_some_and(|a| a.banned) => {
                    st.refuse(
                        conn,
                        "banned",
                        "You've been banned from this server.",
                        false,
                    );
                }
                Some(id) => st.sign_in(conn, id, Some(hash), None, ""),
                None => {
                    debug!(
                        "auth",
                        "Saved sign-in from {ip} wasn't valid (expired or signed out)"
                    );
                    st.refuse(
                        conn,
                        "session_expired",
                        "Your saved sign-in has expired. Sign in again.",
                        false,
                    );
                }
            }
        }
        Some(Auth::Login) => {
            let found = {
                let mut st = shared.lock().unwrap();
                let found = st
                    .accounts
                    .by_name(&name)
                    .map(|a| (a.id, a.password_hash.clone(), a.last_ip == ip));
                let trusted = found.as_ref().is_some_and(|f| f.2);
                if st.failures.name_blocked(&name, trusted) {
                    warn!(
                        "auth",
                        "Refused a sign-in to \"{}\" from {ip}: too many wrong passwords for that name recently",
                        crate::clean(&name, 40)
                    );
                    st.refuse(
                        conn,
                        "too_many_attempts",
                        "Too many wrong passwords for this account. Wait an hour, or ask an admin to reset your password.",
                        true,
                    );
                    return;
                }
                found.map(|f| (f.0, f.1))
            };
            let ok = checked_verify(
                shared,
                &ip,
                &name,
                password,
                found.as_ref().map(|f| f.1.clone()),
            )
            .await;
            let mut st = shared.lock().unwrap();
            match found {
                Some((id, _)) if ok => {
                    if st.accounts.get(id).is_some_and(|a| a.banned) {
                        warn!("auth", "Banned account {name} tried to sign in from {ip}");
                        st.refuse(
                            conn,
                            "banned",
                            "You've been banned from this server.",
                            false,
                        );
                        return;
                    }
                    let (token, hash) = st.accounts.issue_token(id, now_ms()).unwrap();
                    st.save_accounts();
                    st.sign_in(conn, id, Some(hash), Some(token), "");
                }
                _ => {
                    warn!(
                        "auth",
                        "Wrong name or password from {ip} (name \"{}\")",
                        crate::clean(&name, 40)
                    );
                    st.refuse(conn, "bad_login", "Wrong name or password.", true);
                }
            }
        }
        Some(Auth::Register { new_password }) => {
            register(shared, conn, &ip, name, password, new_password).await
        }
        None => legacy(shared, conn, &ip, name, password, accounts_aware).await,
    }
}

async fn register(
    shared: &Shared,
    conn: u32,
    ip: &str,
    name: String,
    invite: String,
    new_password: String,
) {
    let name = {
        let mut st = shared.lock().unwrap();
        if !st.cfg.allow_signup {
            st.refuse(
                conn,
                "signup_closed",
                "This server isn't taking new accounts. Ask whoever runs it.",
                false,
            );
            return;
        }
        let group = st.cfg.password.clone();
        if !group.is_empty() && !ct_eq(invite.as_bytes(), group.as_bytes()) {
            warn!(
                "auth",
                "Wrong group password from {ip} while creating an account"
            );
            st.failures.record(ip, None);
            st.refuse(
                conn,
                "bad_password",
                "That group password isn't right. Ask whoever runs the server for it.",
                true,
            );
            return;
        }
        let name = match accounts::clean_name(&name) {
            Ok(n) => n,
            Err(e) => return st.refuse(conn, "bad_name", &e, false),
        };
        if st.accounts.name_taken(&name, None) {
            return st.refuse(conn, "name_taken", &format!("Someone already has the name \"{name}\". Pick another, or sign in if it's you."), false);
        }
        if let Err(e) = accounts::check_password(&new_password) {
            return st.refuse(conn, "weak_password", &e, false);
        }
        if st.accounts.ip_banned(ip) {
            warn!(
                "auth",
                "Refused a new account \"{name}\" from {ip}: a banned account used that address"
            );
            return st.refuse(
                conn,
                "banned",
                "You can't create an account on this server.",
                false,
            );
        }
        name
    };
    let Some(hash) = limits::hash(new_password).await else {
        return;
    };
    let mut st = shared.lock().unwrap();
    if st.accounts.name_taken(&name, None) {
        // Someone else took it while the password was being hashed.
        return st.refuse(
            conn,
            "name_taken",
            &format!("Someone already has the name \"{name}\". Pick another."),
            false,
        );
    }
    let now = now_ms();
    let id = st.accounts.create(&name, hash, ip, now);
    let (token, token_hash) = st.accounts.issue_token(id, now).unwrap();
    st.save_accounts();
    info!("auth", "{name} created an account (from {ip})");
    st.broadcast_members();
    if st.accounts.admin_count() == 0 && st.accounts.all().len() == 1 {
        warn!(
            "auth",
            "That's the first account. To make yourself an admin, type: admin <your name>"
        );
    }
    st.sign_in(
        conn,
        id,
        Some(token_hash),
        Some(token),
        " with a new account",
    );
}

/// Apps from before accounts send just a name and the group password.
async fn legacy(
    shared: &Shared,
    conn: u32,
    ip: &str,
    name: String,
    password: String,
    accounts_aware: bool,
) {
    let (group_ok, found) = {
        let st = shared.lock().unwrap();
        let group = &st.cfg.password;
        let group_ok = group.is_empty() || ct_eq(password.as_bytes(), group.as_bytes());
        (
            group_ok,
            st.accounts
                .by_name(&name)
                .map(|a| (a.id, a.password_hash.clone(), a.banned)),
        )
    };
    // Someone with an account using an older app: their own password works.
    if let Some((id, hash, banned)) = &found {
        if !group_ok
            && checked_verify(shared, ip, &name, password.clone(), Some(hash.clone())).await
        {
            let mut st = shared.lock().unwrap();
            if *banned {
                return st.refuse(
                    conn,
                    "banned",
                    "You've been banned from this server.",
                    false,
                );
            }
            return st.sign_in(conn, *id, None, None, " (older app)");
        }
    }
    let mut st = shared.lock().unwrap();
    if group_ok {
        debug!(
            "auth",
            "{ip} signed in the old way (name \"{}\"); asked to use an account",
            crate::clean(&name, 40)
        );
        if accounts_aware {
            st.refuse(conn, "account_required", "This server now has accounts. Create yours below with the group password, or sign in if you already have one.", false);
        } else if found.is_some() {
            // Older apps treat "bad_password" as final and stop retrying.
            st.refuse(conn, "bad_password", "This server now has accounts, and that name has one. Get the new TUFFcord with the link below, then sign in.", false);
        } else {
            // The group password alone doesn't let anyone read the chat.
            st.refuse(conn, "bad_password", "This server now has accounts. Get the new TUFFcord with the link below, then create your account.", false);
        }
    } else {
        warn!(
            "auth",
            "Wrong password from {ip} (name \"{}\")",
            crate::clean(&name, 40)
        );
        if found.is_none() {
            st.failures.record(ip, None);
        }
        st.refuse(conn, "bad_password", "That password is not right.", true);
    }
}

// ---------------------------------------------------------------- your own account

pub async fn change_password(shared: &Shared, conn: u32, old: String, new: String) {
    let (account, hash) = {
        let st = shared.lock().unwrap();
        let Some(account) = st.account_of(conn) else {
            return;
        };
        if let Err(e) = accounts::check_password(&new) {
            st.send(
                conn,
                &ServerMsg::Error {
                    code: "weak_password_change".into(),
                    message: e,
                },
            );
            return;
        }
        let Some(a) = st.accounts.get(account) else {
            return;
        };
        // After an admin's reset they're signed in with the temporary password
        // already, so don't ask for it again.
        (
            account,
            (!a.must_change_password).then(|| a.password_hash.clone()),
        )
    };
    if let Some(hash) = hash {
        if !limits::verify(old, Some(hash)).await {
            tokio::time::sleep(Duration::from_millis(800)).await;
            let st = shared.lock().unwrap();
            st.send(
                conn,
                &ServerMsg::Error {
                    code: "bad_old_password".into(),
                    message: "Your current password isn't right.".into(),
                },
            );
            return;
        }
    }
    let Some(new_hash) = limits::hash(new).await else {
        return;
    };
    let mut st = shared.lock().unwrap();
    let now = now_ms();
    let Some(a) = st.accounts.get_mut(account) else {
        return;
    };
    a.password_hash = new_hash;
    a.must_change_password = false;
    a.tokens.clear();
    let me = Account {
        id: a.id,
        name: a.name.clone(),
        admin: a.admin,
    };
    let (token, token_hash) = st.accounts.issue_token(account, now).unwrap();
    st.save_accounts();
    if let Some(c) = st.clients.get_mut(&conn) {
        c.token_hash = Some(token_hash);
    }
    info!("auth", "{} changed their password", me.name);
    let others = st.disconnect_account(
        account,
        "session_expired",
        "Your password was changed, so sign in again.",
        Some(conn),
    );
    st.send(
        conn,
        &ServerMsg::AccountUpdated {
            account: me,
            token: Some(token),
        },
    );
    let text = if others > 0 {
        "Password changed. Your other devices were signed out."
    } else {
        "Password changed."
    };
    st.send(conn, &ServerMsg::Notice { text: text.into() });
}

pub fn rename(st: &mut State, conn: u32, name: &str) {
    let Some(account) = st.account_of(conn) else {
        return;
    };
    let name = match accounts::clean_name(name) {
        Ok(n) => n,
        Err(e) => {
            return st.send(
                conn,
                &ServerMsg::Error {
                    code: "bad_rename".into(),
                    message: e,
                },
            )
        }
    };
    if st.accounts.name_taken(&name, Some(account)) {
        return st.send(
            conn,
            &ServerMsg::Error {
                code: "bad_rename".into(),
                message: format!("Someone already has the name \"{name}\"."),
            },
        );
    }
    let Some(a) = st.accounts.get_mut(account) else {
        return;
    };
    let old = std::mem::replace(&mut a.name, name.clone());
    let me = Account {
        id: a.id,
        name: name.clone(),
        admin: a.admin,
    };
    st.save_accounts();
    info!("auth", "{old} is now called {name}");
    let conns: Vec<u32> = st
        .clients
        .values()
        .filter(|c| c.account == Some(account))
        .map(|c| c.id)
        .collect();
    for id in conns {
        st.clients.get_mut(&id).unwrap().name = name.clone();
        st.send(
            id,
            &ServerMsg::AccountUpdated {
                account: me.clone(),
                token: None,
            },
        );
    }
    st.broadcast_presence();
    st.broadcast_voice();
    st.broadcast_members();
    st.send_accounts_to_admins();
}

pub fn sign_out(st: &mut State, conn: u32) {
    let Some(hash) = st.clients.get_mut(&conn).and_then(|c| c.token_hash.take()) else {
        return;
    };
    st.accounts.revoke_token(&hash);
    st.save_accounts();
    debug!("auth", "{} signed out on one device", st.who(conn));
}

// ---------------------------------------------------------------- admin

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
            warn!("admin", "{by} kicked {name}");
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
            warn!("admin", "{by} banned {name}");
            format!("Banned {name}. They can't sign in or create another account from the same address.")
        }
        Act::Unban => {
            if !t_banned {
                return Err(format!("{name} isn't banned."));
            }
            st.accounts.get_mut(target).unwrap().banned = false;
            st.save_accounts();
            warn!("admin", "{by} unbanned {name}");
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
            warn!("admin", "{by} reset {name}'s password");
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
            warn!(
                "admin",
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
            st.accounts.ensure_owner();
            st.save_accounts();
            warn!("admin", "{by} deleted {name}'s account");
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
        warn!("admin", "The server admin made {name} the owner");
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

/// Startup hint for the host.
pub fn startup_notes(store: &Store) {
    if store.all().is_empty() {
        info!("auth", "No accounts yet. People create one in the app with the group password. Then type \"admin <name>\" here to make yourself an admin.");
    } else if store.admin_count() == 0 {
        warn!(
            "auth",
            "No admins yet. To make yourself one, type: admin <your name>"
        );
    }
}
