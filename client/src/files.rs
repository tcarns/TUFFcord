//! Sending and fetching files over HTTP (servers from 0.7), plus the download
//! cache on disk. See `proto::files` for how the server side works.
//!
//! Everything here blocks, so it runs on worker threads.

use proto::files::{HttpError, NewUpload, UploadCreated, UploadProgress};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// Size of each piece we send (the server takes up to 4 MB).
const PIECE: u64 = 2 * 1024 * 1024;

/// Where to send requests, and the key that says who we are.
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub base: String,
    pub key: String,
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                // Trust what Windows trusts (its certificate store), the same as the
                // chat connection. ureq's default is its own built-in list, which
                // doesn't cover every chain (e.g. some Cloudflare addresses).
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(60 * 30)))
        .timeout_send_body(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .into()
}

pub(crate) fn user_agent() -> String {
    format!("TUFFcord/{}", proto::update::CURRENT)
}

/// Turn an error answer into a sentence (and log exactly what came back).
pub(crate) fn explain(status: u16, body: &str) -> String {
    let snippet: String = body.chars().take(300).collect();
    crate::applog::warn(format!("The server answered {status}: {snippet}"));
    match serde_json::from_str::<HttpError>(body) {
        Ok(e) => e.message,
        Err(_) => format!("The server answered with an error ({status})."),
    }
}

/// What to send: bytes already in memory (a shrunk photo, a poster) or a file.
pub enum Source {
    Bytes(Vec<u8>),
    Path(PathBuf),
}

impl Source {
    pub fn size(&self) -> std::io::Result<u64> {
        match self {
            Source::Bytes(b) => Ok(b.len() as u64),
            Source::Path(p) => std::fs::metadata(p).map(|m| m.len()),
        }
    }
}

/// Upload one file. `progress` gets the bytes sent so far. Returns the upload id
/// (to put in a `Post` message).
pub fn upload(
    to: &Endpoint,
    name: &str,
    source: &Source,
    progress: &dyn Fn(u64),
    cancel: &AtomicBool,
) -> Result<String, String> {
    let started = std::time::Instant::now();
    let size = source.size().unwrap_or(0);
    crate::applog::info(format!(
        "Sending {name} ({}) to {}",
        proto::files::size_label(size),
        to.base
    ));
    let result = upload_inner(to, name, source, progress, cancel);
    match &result {
        Ok(id) => crate::applog::info(format!(
            "Sent {name} in {:.1} s (upload {id})",
            started.elapsed().as_secs_f32()
        )),
        Err(e) => crate::applog::warn(format!("Sending {name} failed: {e}")),
    }
    result
}

fn upload_inner(
    to: &Endpoint,
    name: &str,
    source: &Source,
    progress: &dyn Fn(u64),
    cancel: &AtomicBool,
) -> Result<String, String> {
    let size = source
        .size()
        .map_err(|e| format!("Couldn't read {name} ({e})."))?;
    let agent = agent();
    let body = serde_json::to_string(&NewUpload {
        name: name.to_string(),
        size,
    })
    .unwrap();
    let mut resp = agent
        .post(&format!("{}/files", to.base))
        .header("Authorization", &format!("Bearer {}", to.key))
        .header("User-Agent", &user_agent())
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("Couldn't reach the server ({e})."))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err(explain(status, &text));
    }
    let created: UploadCreated =
        serde_json::from_str(&text).map_err(|_| "The server gave an odd answer.".to_string())?;
    let piece = PIECE.min(created.chunk).max(64 * 1024);

    let mut file = match source {
        Source::Path(p) => Some(File::open(p).map_err(|e| format!("Couldn't read {name} ({e})."))?),
        Source::Bytes(_) => None,
    };
    let mut buf = vec![0u8; piece as usize];
    let mut offset = 0u64;
    let mut failures = 0;
    while offset < size {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled.".into());
        }
        let n = piece.min(size - offset) as usize;
        let chunk: &[u8] = match (&mut file, source) {
            (Some(f), _) => {
                f.seek(SeekFrom::Start(offset))
                    .and_then(|_| f.read_exact(&mut buf[..n]))
                    .map_err(|e| format!("Couldn't read {name} ({e})."))?;
                &buf[..n]
            }
            (None, Source::Bytes(b)) => &b[offset as usize..offset as usize + n],
            (None, Source::Path(_)) => unreachable!(),
        };
        let sent = agent
            .put(&format!("{}/files/{}?offset={offset}", to.base, created.id))
            .header("Authorization", &format!("Bearer {}", to.key))
            .header("User-Agent", &user_agent())
            .header("Content-Type", "application/octet-stream")
            .send(chunk);
        let (status, text) = match sent {
            Ok(mut r) => (
                r.status().as_u16(),
                r.body_mut().read_to_string().unwrap_or_default(),
            ),
            Err(e) => {
                crate::applog::warn(format!(
                    "Sending a piece of {name} failed (will retry): {e}"
                ));
                (0, e.to_string())
            }
        };
        match status {
            200 | 409 => {
                // 409: the server has a different amount; carry on from there.
                let p: UploadProgress = serde_json::from_str(&text)
                    .map_err(|_| "The server gave an odd answer.".to_string())?;
                if status == 409 && p.received == offset {
                    failures += 1;
                } else {
                    failures = 0;
                }
                offset = p.received;
                progress(offset);
                if p.done {
                    return Ok(created.id);
                }
            }
            0 => failures += 1, // connection trouble: try that piece again
            _ => return Err(explain(status, &text)),
        }
        if failures > 0 {
            if failures > 5 {
                return Err(
                    "The upload kept getting interrupted. Check your connection and try again."
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(500 * failures));
        }
    }
    Err("The upload didn't finish.".into())
}

/// Download a file to `dest` (via `dest.part`, picking up where an earlier try
/// stopped). `progress` gets (bytes so far, total).
pub fn download(
    from: &Endpoint,
    id: &str,
    dest: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    let result = download_inner(from, id, dest, progress, cancel);
    if let Err(e) = &result {
        crate::applog::warn(format!(
            "Downloading file {id} from {} failed: {e}",
            from.base
        ));
    }
    result
}

fn download_inner(
    from: &Endpoint,
    id: &str,
    dest: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't save the file ({e})."))?;
    }
    let part = dest.with_extension(format!(
        "{}part",
        dest.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let agent = agent();
    let mut failures = 0;
    loop {
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut req = agent
            .get(&format!("{}/files/{id}", from.base))
            .header("Authorization", &format!("Bearer {}", from.key))
            .header("User-Agent", &user_agent());
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
        }
        let result = (|| -> Result<bool, (bool, String)> {
            let mut resp = req
                .call()
                .map_err(|e| (true, format!("Couldn't reach the server ({e}).")))?;
            let status = resp.status().as_u16();
            if status != 200 && status != 206 {
                let text = resp.body_mut().read_to_string().unwrap_or_default();
                if status == 416 {
                    // We already have it all.
                    return Ok(true);
                }
                return Err((false, explain(status, &text)));
            }
            let len: u64 = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let (mut file, start) = if status == 206 {
                let f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&part)
                    .map_err(|e| (false, format!("Couldn't save the file ({e}).")))?;
                (f, have)
            } else {
                (
                    File::create(&part)
                        .map_err(|e| (false, format!("Couldn't save the file ({e}).")))?,
                    0,
                )
            };
            let total = start + len;
            let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
            let mut buf = vec![0u8; 64 * 1024];
            let mut done = start;
            let mut last = std::time::Instant::now();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err((false, "Cancelled.".into()));
                }
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| (true, format!("The download was interrupted ({e}).")))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])
                    .map_err(|e| (false, format!("Couldn't save the file ({e}).")))?;
                done += n as u64;
                if last.elapsed() > Duration::from_millis(100) {
                    progress(done, total);
                    last = std::time::Instant::now();
                }
            }
            progress(done, total);
            Ok(len == 0 || done >= total)
        })();
        match result {
            Ok(true) => {
                std::fs::rename(&part, dest)
                    .map_err(|e| format!("Couldn't save the file ({e})."))?;
                return Ok(());
            }
            Ok(false) | Err((true, _)) if failures < 5 => {
                failures += 1;
                crate::applog::warn(format!(
                    "Download of {id} interrupted; trying again ({failures})"
                ));
                std::thread::sleep(Duration::from_millis(700 * failures));
            }
            Ok(false) => return Err("The download didn't finish. Try again.".into()),
            Err((_, e)) => {
                if e != "Cancelled." && !e.contains("interrupted") {
                    let _ = std::fs::remove_file(&part);
                }
                return Err(e);
            }
        }
    }
}

// ---------------------------------------------------------------- cache

/// Where downloaded files are kept: %LOCALAPPDATA%\TUFFcord\cache on Windows.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("TUFFcord")
        .join("cache")
}

/// The cached copy of attachment `id`. The extension is kept so Windows (and
/// its video player) can tell what the file is.
pub fn cached_path(id: &str, name: &str) -> PathBuf {
    let ext = proto::files::extension(name);
    let mut safe_id: String = id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    if safe_id.is_empty() {
        safe_id.push('0');
    }
    if ext.is_empty() {
        cache_dir().join(safe_id)
    } else {
        cache_dir().join(format!("{safe_id}.{ext}"))
    }
}

/// Space the downloaded files use now, in bytes.
pub fn cache_used() -> u64 {
    std::fs::read_dir(cache_dir())
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Keep the cache under `budget` bytes, removing the least recently used
/// files. Also clears unfinished downloads more than a day old.
pub fn prune_cache(budget: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let t = m.accessed().or_else(|_| m.modified()).ok()?;
            Some((e.path(), m.len(), t))
        })
        .collect();
    let day = Duration::from_secs(24 * 3600);
    files.retain(|(p, _, t)| {
        let stale_part =
            p.extension().is_some_and(|e| e == "part") && t.elapsed().is_ok_and(|age| age > day);
        if stale_part {
            let _ = std::fs::remove_file(p);
        }
        !stale_part
    });
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    for (path, len, _) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Mark a file as downloaded from the internet, so Windows warns before
/// running it (the same "Mark of the Web" a browser adds).
pub fn mark_downloaded(path: &Path) {
    #[cfg(windows)]
    {
        let mut ads = path.as_os_str().to_os_string();
        ads.push(":Zone.Identifier");
        let _ = std::fs::write(ads, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Copy a cached file to a temporary folder under its real name and open it
/// with the program Windows uses for that kind of file.
pub fn open_with_system(cached: &Path, name: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join("TUFFcord");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(proto::files::clean_name(name));
    std::fs::copy(cached, &target).map_err(|e| e.to_string())?;
    mark_downloaded(&target);
    open_path(&target)
}

pub fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(not(windows))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// Copy a cached file to where the person chose, marked as downloaded.
pub fn save_copy(cached: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(cached, to).map_err(|e| e.to_string())?;
    mark_downloaded(to);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_names() {
        let p = cached_path("abc123", "Funny Clip.MP4");
        assert_eq!(p.file_name().unwrap(), "abc123.mp4");
        let p = cached_path("../../x", "a");
        assert_eq!(p.file_name().unwrap(), "0");
        assert_eq!(p.parent().unwrap(), cache_dir());
        assert!(cached_path("ff", "noext").ends_with("ff"));
    }
}
