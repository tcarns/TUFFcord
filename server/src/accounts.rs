//! Accounts: names, password hashes, saved sign-ins, admin and ban flags.
//!
//! Saved in `data/accounts.json` next to the chat history. Passwords are stored
//! only as Argon2id hashes, and saved sign-ins ("tokens") only as SHA-256 hashes,
//! so the file can't be used to sign in as anyone. Still, keep it private.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const MIN_PASSWORD: usize = 8;
pub const MAX_PASSWORD: usize = 128;
pub const MAX_NAME: usize = 32;
/// Saved sign-ins unused this long stop working.
const TOKEN_LIFETIME_MS: u64 = 180 * 24 * 60 * 60 * 1000;
/// Saved sign-ins kept per account (the least recently used go first).
const MAX_TOKENS: usize = 10;

#[derive(Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct FileData {
    next_id: u32,
    accounts: Vec<Account>,
    /// One-time invites admins made (see `invites.rs`).
    invites: Vec<Invite>,
    /// Recent admin actions, newest last (see `adminlog.rs`).
    admin_log: Vec<LogEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    /// SHA-256 of the code (the code itself is only shown to the admin once).
    pub hash: String,
    pub by: u32,
    pub expires: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub ts: u64,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: u32,
    pub name: String,
    pub password_hash: String,
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub banned: bool,
    /// The one admin other admins can't act on; only they (and the server
    /// window) can give or take admin, or reset or delete another admin.
    #[serde(default)]
    pub owner: bool,
    /// Signed in with a temporary password from an admin; must pick a new one.
    #[serde(default)]
    pub must_change_password: bool,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub last_seen: u64,
    #[serde(default)]
    pub last_ip: String,
    #[serde(default)]
    pub tokens: Vec<Token>,
    /// "New sign-in from ..." notes not yet shown on another of their devices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    /// SHA-256 of the token, in hex.
    pub hash: String,
    pub created: u64,
    pub last_used: u64,
    /// Address it was last used from (for the devices list).
    #[serde(default)]
    pub ip: String,
}

pub struct Store {
    path: PathBuf,
    data: FileData,
    /// Changed since the last save (only for small things like "last seen";
    /// anything important is saved right away).
    pub dirty: bool,
}

// ---------------------------------------------------------------- names and passwords

/// Tidy a name and check it's usable: 2–32 characters, letters, numbers,
/// spaces and `_ - . '`, with at least one letter or number.
pub fn clean_name(raw: &str) -> Result<String, String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = name.chars().count();
    if len < 2 {
        return Err("Names need at least 2 characters.".into());
    }
    if len > MAX_NAME {
        return Err(format!("Names can be up to {MAX_NAME} characters."));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.' | '\'')))
    {
        return Err(format!(
            "Names can't contain \"{bad}\". Use letters, numbers, spaces, _ - . or '."
        ));
    }
    if !name.chars().any(char::is_alphanumeric) {
        return Err("Names need at least one letter or number.".into());
    }
    if mixed_scripts(&name) {
        return Err(
            "Names can't mix alphabets (like Latin and Cyrillic letters that look alike).".into(),
        );
    }
    Ok(name)
}

/// Names match regardless of upper/lower case.
/// Letters from more than one of Latin, Greek and Cyrillic: how look-alike
/// names are made ("Tyler" with a Cyrillic "е").
fn mixed_scripts(name: &str) -> bool {
    let mut seen = [false; 3];
    for c in name.chars().filter(|c| c.is_alphabetic()) {
        match c as u32 {
            0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF => seen[0] = true,
            0x370..=0x3FF | 0x1F00..=0x1FFF => seen[1] = true,
            0x400..=0x52F => seen[2] = true,
            _ => {}
        }
    }
    seen.iter().filter(|s| **s).count() > 1
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

pub fn check_password(pw: &str) -> Result<(), String> {
    let n = pw.chars().count();
    if n < MIN_PASSWORD {
        Err(format!(
            "Passwords need at least {MIN_PASSWORD} characters."
        ))
    } else if n > MAX_PASSWORD {
        Err(format!("Passwords can be up to {MAX_PASSWORD} characters."))
    } else {
        Ok(())
    }
}

/// Slow on purpose (~50 ms, 19 MB briefly): run it off the voice thread.
pub fn hash_password(pw: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("system random source unavailable");
    let salt = SaltString::encode_b64(&salt).expect("salt");
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .expect("hash password")
        .to_string()
}

/// Slow on purpose: run it off the voice thread. With no hash (unknown name),
/// checks against a stand-in so a wrong name takes as long as a wrong password.
pub fn verify_password(pw: &str, hash: Option<&str>) -> bool {
    static STAND_IN: OnceLock<String> = OnceLock::new();
    let real = hash.is_some();
    let hash = hash.unwrap_or_else(|| STAND_IN.get_or_init(|| hash_password("stand-in")));
    let ok = PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false);
    ok && real
}

/// A new sign-in token: (what the app keeps, what the server keeps).
pub fn new_token() -> (String, String) {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("system random source unavailable");
    let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn is_loopback(ip: &str) -> bool {
    ip.parse::<std::net::IpAddr>()
        .map(|a| a.is_loopback())
        .unwrap_or(false)
}

// ---------------------------------------------------------------- the store

impl Store {
    pub fn path_in(data_dir: &Path) -> PathBuf {
        data_dir.join("accounts.json")
    }

    /// Read the accounts file. A file that exists but can't be read is an error
    /// (starting empty would let anyone take over names and lose the admins).
    pub fn load(data_dir: &Path) -> Result<Store, String> {
        let path = Self::path_in(data_dir);
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| {
                format!(
                    "The accounts file {} is damaged ({e}). Fix or restore it, or move it away to start with no accounts.",
                    path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileData::default(),
            Err(e) => return Err(format!("Couldn't read {} ({e}).", path.display())),
        };
        let mut store = Store {
            path,
            data,
            dirty: false,
        };
        store.ensure_owner();
        Ok(store)
    }

    /// Servers from before 0.10.1 have admins but no owner: the first admin
    /// (the one who made themselves admin from the server window) becomes it.
    /// Also after the owner's account was deleted.
    pub fn ensure_owner(&mut self) {
        if self.data.accounts.iter().any(|a| a.owner) {
            return;
        }
        if let Some(a) = self
            .data
            .accounts
            .iter_mut()
            .filter(|a| a.admin)
            .min_by_key(|a| a.id)
        {
            a.owner = true;
            self.dirty = true;
        }
    }

    pub fn owner(&self) -> Option<u32> {
        self.data.accounts.iter().find(|a| a.owner).map(|a| a.id)
    }

    /// Make `id` the owner (and an admin); the previous owner stays an admin.
    pub fn set_owner(&mut self, id: u32) {
        for a in &mut self.data.accounts {
            a.owner = a.id == id;
            if a.owner {
                a.admin = true;
            }
        }
        self.dirty = true;
    }

    pub fn save(&mut self) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = serde_json::to_vec_pretty(&self.data).expect("serialize accounts");
        std::fs::write(&tmp, json)
            .and_then(|_| std::fs::rename(&tmp, &self.path))
            .map_err(|e| format!("Saving accounts failed: {e}"))?;
        self.dirty = false;
        Ok(())
    }

    pub fn all(&self) -> &[Account] {
        &self.data.accounts
    }

    pub fn get(&self, id: u32) -> Option<&Account> {
        self.data.accounts.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Account> {
        self.data.accounts.iter_mut().find(|a| a.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Account> {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        self.data
            .accounts
            .iter()
            .find(|a| same_name(&a.name, &name))
    }

    /// Someone other than `except` already has this name.
    pub fn name_taken(&self, name: &str, except: Option<u32>) -> bool {
        self.data
            .accounts
            .iter()
            .any(|a| Some(a.id) != except && same_name(&a.name, name))
    }

    pub fn admin_count(&self) -> usize {
        self.data.accounts.iter().filter(|a| a.admin).count()
    }

    /// A banned account last used this address (local addresses never count,
    /// so banning someone on the host's PC doesn't lock the host out).
    pub fn ip_banned(&self, ip: &str) -> bool {
        !ip.is_empty()
            && !is_loopback(ip)
            && self
                .data
                .accounts
                .iter()
                .any(|a| a.banned && a.last_ip == ip)
    }

    pub fn create(&mut self, name: &str, password_hash: String, ip: &str, now: u64) -> u32 {
        self.data.next_id = self.data.next_id.max(1);
        let id = self.data.next_id;
        self.data.next_id += 1;
        self.data.accounts.push(Account {
            id,
            name: name.to_string(),
            password_hash,
            admin: false,
            banned: false,
            owner: false,
            must_change_password: false,
            created: now,
            last_seen: now,
            last_ip: ip.to_string(),
            tokens: Vec::new(),
            notices: Vec::new(),
        });
        id
    }

    pub fn delete(&mut self, id: u32) -> Option<Account> {
        let i = self.data.accounts.iter().position(|a| a.id == id)?;
        Some(self.data.accounts.remove(i))
    }

    /// Hand out a new saved sign-in for this account. Returns the token to give the app.
    pub fn issue_token(&mut self, id: u32, now: u64, ip: &str) -> Option<(String, String)> {
        let a = self.get_mut(id)?;
        let (token, hash) = new_token();
        a.tokens
            .retain(|t| now.saturating_sub(t.last_used) < TOKEN_LIFETIME_MS);
        a.tokens.push(Token {
            hash: hash.clone(),
            created: now,
            last_used: now,
            ip: ip.to_string(),
        });
        if a.tokens.len() > MAX_TOKENS {
            a.tokens.sort_by_key(|t| std::cmp::Reverse(t.last_used));
            a.tokens.truncate(MAX_TOKENS);
        }
        Some((token, hash))
    }

    /// The account a saved sign-in belongs to, if it's still good.
    pub fn use_token(&mut self, hash: &str, now: u64, ip: &str) -> Option<u32> {
        for a in &mut self.data.accounts {
            if let Some(t) = a.tokens.iter_mut().find(|t| t.hash == hash) {
                if now.saturating_sub(t.last_used) >= TOKEN_LIFETIME_MS {
                    a.tokens.retain(|t| t.hash != hash);
                    self.dirty = true;
                    return None;
                }
                t.last_used = now;
                t.ip = ip.to_string();
                self.dirty = true;
                return Some(a.id);
            }
        }
        None
    }

    pub fn invites(&mut self) -> &mut Vec<Invite> {
        &mut self.data.invites
    }

    pub fn admin_log(&mut self) -> &mut Vec<LogEntry> {
        &mut self.data.admin_log
    }

    pub fn revoke_token(&mut self, hash: &str) {
        for a in &mut self.data.accounts {
            a.tokens.retain(|t| t.hash != hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("  Big   Tony ").unwrap(), "Big Tony");
        assert_eq!(clean_name("o'neil_2.0-x").unwrap(), "o'neil_2.0-x");
        assert_eq!(clean_name("Zoë").unwrap(), "Zoë");
        assert!(clean_name("a").is_err());
        assert!(clean_name("__").is_err());
        assert!(clean_name("tab\there").is_ok()); // whitespace collapses to a space
        assert!(clean_name("no<html>").is_err());
        assert!(clean_name(&"x".repeat(33)).is_err());
        assert!(
            clean_name("Tyl\u{0435}r").is_err(),
            "Cyrillic e in a Latin name"
        );
        assert!(
            clean_name("\u{0418}\u{0432}\u{0430}\u{043d}").is_ok(),
            "all Cyrillic is fine"
        );
        assert!(clean_name("Zoë 2").is_ok());
        assert!(check_password("1234567").is_err());
        assert!(check_password("12345678").is_ok());
    }

    #[test]
    fn passwords_and_tokens() {
        let h = hash_password("hunter22");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("hunter22", Some(&h)));
        assert!(!verify_password("hunter23", Some(&h)));
        assert!(!verify_password("stand-in", None));

        let dir = std::env::temp_dir().join(format!("br-accounts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(&dir).unwrap();
        let id = s.create("Sam", h, "1.2.3.4", 1000);
        let other = s.create("Pat", hash_password("patpass11"), "", 1000);
        assert_eq!(s.owner(), None);
        s.get_mut(other).unwrap().admin = true;
        s.ensure_owner();
        assert_eq!(s.owner(), Some(other), "the first admin becomes the owner");
        s.set_owner(id);
        assert!(s.get(id).unwrap().admin && !s.get(other).unwrap().owner);
        s.set_owner(other);
        s.get_mut(id).unwrap().admin = false;
        assert!(s.name_taken("sam", None));
        assert!(!s.name_taken("SAM", Some(id)));
        assert_eq!(s.by_name("  SAM ").unwrap().id, id);

        let (tok, hash) = s.issue_token(id, 1000, "").unwrap();
        assert_eq!(token_hash(&tok), hash);
        assert_eq!(s.use_token(&hash, 2000, ""), Some(id));
        assert_eq!(s.use_token(&token_hash("nope"), 2000, ""), None);
        // Expires after going unused for the lifetime.
        assert_eq!(s.use_token(&hash, 2000 + TOKEN_LIFETIME_MS, ""), None);
        assert_eq!(s.use_token(&hash, 2000, ""), None);

        for i in 0..15 {
            s.issue_token(id, 3000 + i, "");
        }
        assert_eq!(s.get(id).unwrap().tokens.len(), MAX_TOKENS);

        s.get_mut(id).unwrap().banned = true;
        assert!(s.ip_banned("1.2.3.4"));
        assert!(!s.ip_banned("5.6.7.8"));
        s.get_mut(id).unwrap().last_ip = "127.0.0.1".into();
        assert!(!s.ip_banned("127.0.0.1"));

        s.save().unwrap();
        let again = Store::load(&dir).unwrap();
        assert_eq!(again.all().len(), 2);
        assert_eq!(again.all()[0].tokens.len(), MAX_TOKENS);
        std::fs::write(Store::path_in(&dir), "{oops").unwrap();
        assert!(Store::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
