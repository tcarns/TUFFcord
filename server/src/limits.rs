//! Limits that keep strangers from guessing passwords or running the server
//! out of memory: wrong-password counts (per address and per account), how
//! many password hashes run at once, and how many connections are open.

use crate::accounts;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Wrong passwords allowed from one address in `IP_WINDOW`.
const IP_MAX: usize = 10;
const IP_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Wrong passwords allowed for one name in `NAME_WINDOW`, from addresses that
/// haven't signed in to it before (so guessing from many addresses is bounded too).
const NAME_MAX: usize = 20;
const NAME_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Addresses and names remembered at most (oldest dropped), so the lists can't grow without end.
const MAX_KEYS: usize = 10_000;

/// Each password hash takes ~19 MB for a moment; this many run at once, the
/// rest wait their turn.
static HASHES: Semaphore = Semaphore::const_new(2);

/// Open connections (app and file transfers) at most.
pub const MAX_CONNECTIONS: usize = 256;
/// App connections from one address that haven't signed in yet, at most.
pub const MAX_UNSIGNED_PER_IP: usize = 8;
static OPEN: AtomicUsize = AtomicUsize::new(0);

/// WebSocket limits: the biggest message is an image sent the old way (apps
/// before 0.7, 16 MB). Frames stay small so nobody buffers a huge one.
pub fn websocket_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(17 * 1024 * 1024))
        .max_frame_size(Some(1024 * 1024))
}

/// Counts an open connection while it lives. None when the server is full.
pub struct Slot(());

impl Slot {
    pub fn take() -> Option<Slot> {
        let before = OPEN.fetch_add(1, Ordering::Relaxed);
        if before >= MAX_CONNECTIONS {
            OPEN.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        Some(Slot(()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::Relaxed);
    }
}

pub async fn hash(pw: String) -> Option<String> {
    let _turn = HASHES.acquire().await.ok()?;
    tokio::task::spawn_blocking(move || accounts::hash_password(&pw))
        .await
        .ok()
}

pub async fn verify(pw: String, hash: Option<String>) -> bool {
    let Ok(_turn) = HASHES.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || accounts::verify_password(&pw, hash.as_deref()))
        .await
        .unwrap_or(false)
}

#[derive(Default)]
pub struct Failures {
    by_ip: HashMap<String, VecDeque<Instant>>,
    by_name: HashMap<String, VecDeque<Instant>>,
}

fn prune(map: &mut HashMap<String, VecDeque<Instant>>, window: Duration) {
    let now = Instant::now();
    map.retain(|_, l| {
        while l.front().is_some_and(|t| now.duration_since(*t) > window) {
            l.pop_front();
        }
        !l.is_empty()
    });
    if map.len() > MAX_KEYS {
        let mut oldest: Vec<(Instant, String)> = map
            .iter()
            .map(|(k, l)| (*l.back().unwrap(), k.clone()))
            .collect();
        oldest.sort();
        for (_, k) in oldest.into_iter().take(map.len() - MAX_KEYS) {
            map.remove(&k);
        }
    }
}

fn push(map: &mut HashMap<String, VecDeque<Instant>>, key: &str, max: usize) {
    let l = map.entry(key.to_string()).or_default();
    l.push_back(Instant::now());
    while l.len() > max * 2 {
        l.pop_front();
    }
}

fn name_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl Failures {
    pub fn ip_blocked(&mut self, ip: &str) -> bool {
        prune(&mut self.by_ip, IP_WINDOW);
        self.by_ip.get(ip).is_some_and(|l| l.len() >= IP_MAX)
    }

    /// `trusted`: the address last signed in to this account, which the
    /// per-name limit leaves alone (so a stranger can't lock the owner out).
    pub fn name_blocked(&mut self, name: &str, trusted: bool) -> bool {
        prune(&mut self.by_name, NAME_WINDOW);
        !trusted
            && self
                .by_name
                .get(&name_key(name))
                .is_some_and(|l| l.len() >= NAME_MAX)
    }

    /// Count a wrong password. Called *before* the slow check (and taken back
    /// with `forgive` if it was right), so guesses sent all at once still count.
    pub fn record(&mut self, ip: &str, name: Option<&str>) {
        push(&mut self.by_ip, ip, IP_MAX);
        if let Some(n) = name {
            push(&mut self.by_name, &name_key(n), NAME_MAX);
        }
    }

    pub fn forgive(&mut self, ip: &str, name: Option<&str>) {
        if let Some(l) = self.by_ip.get_mut(ip) {
            l.pop_back();
        }
        if let Some(l) = name.and_then(|n| self.by_name.get_mut(&name_key(n))) {
            l.pop_back();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_forgives() {
        let mut f = Failures::default();
        for _ in 0..IP_MAX {
            f.record("1.2.3.4", Some("Alice"));
        }
        assert!(f.ip_blocked("1.2.3.4"));
        assert!(!f.ip_blocked("1.2.3.5"));
        f.forgive("1.2.3.4", Some("alice"));
        assert!(!f.ip_blocked("1.2.3.4"));
        for i in 0..NAME_MAX {
            f.record(&format!("10.0.0.{i}"), Some(" ALICE "));
        }
        assert!(f.name_blocked("alice", false));
        assert!(
            !f.name_blocked("alice", true),
            "the account's own address isn't locked out"
        );
        assert!(!f.name_blocked("bob", false));
    }

    #[test]
    fn slots() {
        let held: Vec<Slot> = (0..MAX_CONNECTIONS).map_while(|_| Slot::take()).collect();
        assert_eq!(held.len(), MAX_CONNECTIONS);
        assert!(Slot::take().is_none());
        drop(held);
        assert!(Slot::take().is_some());
    }
}
