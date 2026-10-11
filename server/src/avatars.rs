//! Profile pictures: one small PNG per account in `data/avatars/<account>.png`.
//! `PUT /avatar` sets yours, `DELETE /avatar` removes it, `GET /avatars/<account>`
//! fetches one. The member list carries each picture's hash, so apps fetch a
//! picture only when it changes.

use crate::http::{body_size, error, read_body, read_chunked, respond, who, BodySize, Request};
use crate::{Config, Shared};
use proto::files::{AVATAR_SIDE, MAX_AVATAR};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

/// Changes one account can make this often (each one updates everyone's list).
const COOLDOWN: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Avatars {
    /// Account -> hash of its picture.
    hashes: HashMap<u32, String>,
    last_change: HashMap<u32, Instant>,
}

pub fn dir(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("avatars")
}

fn path(cfg: &Config, account: u32) -> PathBuf {
    dir(cfg).join(format!("{account}.png"))
}

fn hash_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A PNG of the right size, or why not.
fn check(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() > MAX_AVATAR {
        return Err("The picture is too big.");
    }
    // Signature, then the IHDR chunk with width and height.
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err("The picture isn't a PNG.");
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if w != AVATAR_SIDE || h != AVATAR_SIDE {
        return Err("The picture is the wrong size.");
    }
    Ok(())
}

impl Avatars {
    /// The pictures already saved.
    pub fn load(cfg: &Config) -> Self {
        let mut hashes = HashMap::new();
        for e in std::fs::read_dir(dir(cfg)).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".png").and_then(|n| n.parse().ok()) else {
                continue;
            };
            if let Ok(bytes) = std::fs::read(e.path()) {
                hashes.insert(id, hash_of(&bytes));
            }
        }
        Avatars {
            hashes,
            ..Default::default()
        }
    }

    pub fn hash(&self, account: u32) -> Option<String> {
        self.hashes.get(&account).cloned()
    }

    /// The account was deleted.
    pub fn remove(&mut self, cfg: &Config, account: u32) {
        if self.hashes.remove(&account).is_some() {
            let _ = std::fs::remove_file(path(cfg, account));
        }
    }

    /// Too soon after this account's last change?
    fn too_soon(&mut self, account: u32) -> bool {
        let now = Instant::now();
        match self.last_change.get(&account) {
            Some(t) if now.duration_since(*t) < COOLDOWN => true,
            _ => {
                self.last_change.insert(account, now);
                false
            }
        }
    }
}

/// `PUT /avatar` (a body) or `DELETE /avatar` (none).
pub async fn set(stream: &mut TcpStream, req: &Request, body: Vec<u8>, shared: &Shared) {
    let account = who(&shared.lock().unwrap(), req).map(|(_, a)| a);
    let Some(account) = account else {
        return error(stream, req, 401, "signed_out", "Sign in again.").await;
    };
    let picture = if req.method == "DELETE" {
        None
    } else {
        let bytes = match body_size(req) {
            Some(BodySize::Known(n)) if n as usize <= MAX_AVATAR => {
                read_body(stream, body, n).await
            }
            Some(BodySize::Chunked) => read_chunked(stream, body, MAX_AVATAR).await.ok(),
            Some(_) => return error(stream, req, 413, "too_big", "The picture is too big.").await,
            None => return error(stream, req, 411, "length_required", "Unreadable request.").await,
        };
        let Some(bytes) = bytes else {
            return error(
                stream,
                req,
                400,
                "bad_request",
                "The picture didn't arrive.",
            )
            .await;
        };
        if let Err(why) = check(&bytes) {
            return error(stream, req, 400, "bad_picture", why).await;
        }
        Some(bytes)
    };
    let cfg = {
        let mut st = shared.lock().unwrap();
        (!st.avatars.too_soon(account)).then(|| st.cfg.clone())
    };
    let Some(cfg) = cfg else {
        return error(
            stream,
            req,
            429,
            "slow_down",
            "Wait a moment and try again.",
        )
        .await;
    };
    let file = path(&cfg, account);
    let saved = match &picture {
        Some(bytes) => {
            let _ = tokio::fs::create_dir_all(dir(&cfg)).await;
            tokio::fs::write(&file, bytes).await
        }
        None => match tokio::fs::remove_file(&file).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    };
    if let Err(e) = saved {
        warn!("avatars", "Couldn't save account {account}'s picture: {e}");
        return error(
            stream,
            req,
            507,
            "not_saved",
            "The server couldn't save it.",
        )
        .await;
    }
    {
        let mut st = shared.lock().unwrap();
        match &picture {
            Some(bytes) => st.avatars.hashes.insert(account, hash_of(bytes)),
            None => st.avatars.hashes.remove(&account),
        };
        let name = st
            .accounts
            .get(account)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        info!(
            "avatars",
            "{name} {} their profile picture",
            if picture.is_some() {
                "changed"
            } else {
                "removed"
            }
        );
        st.broadcast_members();
    }
    respond(stream, 200, "application/json", b"{}").await;
}

/// `GET /avatars/<account>`.
pub async fn get(stream: &mut TcpStream, req: &Request, account: &str, shared: &Shared) {
    let file = {
        let st = shared.lock().unwrap();
        if who(&st, req).is_none() {
            None.ok_or(401)
        } else {
            account
                .parse::<u32>()
                .ok()
                .filter(|a| st.avatars.hashes.contains_key(a))
                .map(|a| path(&st.cfg, a))
                .ok_or(404)
        }
    };
    let bytes = match file {
        Ok(f) => tokio::fs::read(f).await.ok(),
        Err(401) => return error(stream, req, 401, "signed_out", "Sign in again.").await,
        Err(_) => None,
    };
    match bytes {
        Some(b) => respond(stream, 200, "image/png", &b).await,
        None => error(stream, req, 404, "gone", "No picture.").await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b
    }

    #[test]
    fn checks_pictures() {
        assert!(check(&png(64, 64)).is_ok());
        assert!(check(&png(64, 65)).is_err());
        assert!(check(b"GIF89a, not a png at all").is_err());
        let mut big = png(64, 64);
        big.resize(MAX_AVATAR + 1, 0);
        assert!(check(&big).is_err());
    }
}
