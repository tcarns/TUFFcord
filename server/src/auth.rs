//! Signing in, creating accounts, and account management (by the person
//! themselves, by admins in the app, and from the server window).
//!
//! Password hashing is slow on purpose, so it runs on tokio's blocking pool
//! with the state unlocked; voice keeps flowing for everyone meanwhile.

use crate::accounts::{self, Store};
use crate::{leave_voice, limits, now_ms, Shared, State};
use proto::{Account, AccountSummary, Auth, Member, ServerMsg, FEATURE_ACCOUNTS};
use proto::{
    FEATURE_ADMIN_LOG, FEATURE_ATTACHMENTS, FEATURE_CHANNELS, FEATURE_CHANNEL_EDIT, FEATURE_FILES,
    FEATURE_OLDER_MESSAGES, FEATURE_SESSIONS, PROTOCOL_VERSION,
};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------- helpers on State

impl State {
    pub(crate) fn account_of(&self, conn: u32) -> Option<u32> {
        self.clients.get(&conn).and_then(|c| c.account)
    }

    pub(crate) fn summaries(&self) -> Vec<AccountSummary> {
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
                avatar: self.avatars.hash(a.id),
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
    pub(crate) fn refuse(&self, conn: u32, code: &str, message: &str, slow: bool) {
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

    pub(crate) fn save_accounts(&mut self) {
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
                FEATURE_SESSIONS.to_string(),
                FEATURE_ADMIN_LOG.to_string(),
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
            match st.accounts.use_token(&hash, now_ms(), &ip) {
                Some(id) if st.accounts.get(id).is_some_and(|a| a.banned) => {
                    st.refuse(
                        conn,
                        "banned",
                        "You've been banned from this server.",
                        false,
                    );
                }
                Some(id) => {
                    st.sign_in(conn, id, Some(hash), None, "");
                    crate::sessions::deliver_notices(&mut st, conn, id);
                }
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
                    crate::sessions::note_new_address(&mut st, id, &ip);
                    let (token, hash) = st.accounts.issue_token(id, now_ms(), &ip).unwrap();
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
        if !crate::invites::valid(&mut st, &invite) {
            warn!(
                "auth",
                "Wrong or used invite from {ip} while creating an account"
            );
            st.failures.record(ip, None);
            st.refuse(
                conn,
                "bad_password",
                "That invite isn't right, was already used, or is over 24 hours old. Ask whoever runs the server for a new one.",
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
    if !crate::invites::redeem(&mut st, &invite) {
        return st.refuse(
            conn,
            "bad_password",
            "That invite was just used. Ask for another.",
            false,
        );
    }
    let now = now_ms();
    let id = st.accounts.create(&name, hash, ip, now);
    let (token, token_hash) = st.accounts.issue_token(id, now, ip).unwrap();
    st.save_accounts();
    info!(
        "auth",
        "{name} created an account with an invite (from {ip})"
    );
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

/// Apps from before accounts send just a name and a password. Their account's
/// own password still works; anything else is told how to get in now.
async fn legacy(
    shared: &Shared,
    conn: u32,
    ip: &str,
    name: String,
    password: String,
    accounts_aware: bool,
) {
    let found = shared
        .lock()
        .unwrap()
        .accounts
        .by_name(&name)
        .map(|a| (a.id, a.password_hash.clone(), a.banned));
    if let Some((id, hash, banned)) = &found {
        if checked_verify(shared, ip, &name, password, Some(hash.clone())).await {
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
    let st = shared.lock().unwrap();
    debug!(
        "auth",
        "{ip} signed in the old way (name \"{}\"); asked to use an account",
        crate::clean(&name, 40)
    );
    if accounts_aware {
        st.refuse(conn, "account_required", "This server needs an account. Sign in, or choose Create account and paste the invite code you were given.", false);
    } else {
        // Older apps treat "bad_password" as final and stop retrying.
        st.refuse(conn, "bad_password", "This server needs an account now. Get the new TUFFcord with the link below, then sign in or create one with an invite.", false);
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
    let ip = st
        .clients
        .get(&conn)
        .map(|c| c.ip.clone())
        .unwrap_or_default();
    let (token, token_hash) = st.accounts.issue_token(account, now, &ip).unwrap();
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

/// Startup hint for the host.
pub fn startup_notes(store: &Store) {
    if store.all().is_empty() {
        info!("auth", "No accounts yet. Type \"invite\" here for an invite code, create your account with it in the app, then type \"admin <your name>\" to make yourself an admin.");
    } else if store.admin_count() == 0 {
        warn!(
            "auth",
            "No admins yet. To make yourself one, type: admin <your name>"
        );
    }
}
