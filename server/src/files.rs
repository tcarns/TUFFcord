//! Files over plain HTTP on the server's port (see `proto::files`), so a big
//! video never holds up anyone's voice on the WebSocket.
//!
//! Uploads are written to `data/attachments/<id>.part` as they arrive and renamed
//! to `<id>` when complete. A finished upload belongs to whoever sent it until a
//! `Post` message attaches it to a chat message; unclaimed ones are deleted after
//! half an hour. What a file is (image, video…) is decided here from its first
//! bytes, never from the sender's word.

use crate::http::{
    body_size, error, json, read_body, read_chunked, valid_id, who, BodySize, Request, IDLE,
};
use crate::{attachments_dir, history, now_ms, random_id, Shared, State};
use proto::files::{self, NewUpload, UploadCreated, UploadProgress, CHUNK, MAX_POSTER};
use proto::{Attachment, ChatMessage, PostFile, Poster, ServerMsg};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Unclaimed uploads are deleted after this long.
const UPLOAD_LIFETIME: Duration = Duration::from_secs(30 * 60);
/// Unfinished uploads one person can have at once.
const MAX_OPEN_UPLOADS: usize = 12;
/// New uploads one person can start per minute.
const UPLOADS_PER_MINUTE: usize = 30;
/// Files in one message.
pub const MAX_FILES_PER_POST: usize = 10;
/// Apps before 0.7 download over the WebSocket; that's only allowed for images
/// up to this size, so nobody's voice stalls behind a big file.
pub const MAX_WS_BYTES: u64 = 16 * 1024 * 1024;

pub struct Upload {
    pub owner: u32,
    pub name: String,
    pub size: u64,
    pub received: u64,
    /// First bytes, to tell what the file is.
    head: Vec<u8>,
    pub mime: &'static str,
    pub done: bool,
    /// A request is writing to it right now.
    busy: bool,
    created: Instant,
}

#[derive(Default)]
pub struct Files {
    pub uploads: HashMap<String, Upload>,
    /// File key (from `Welcome`) -> connection.
    pub keys: HashMap<String, u32>,
    /// Bytes in data/attachments (finished files).
    pub stored: u64,
    /// Recent upload starts per account, for the per-minute limit.
    starts: HashMap<u32, Vec<Instant>>,
    /// Bytes each account started sending in the last day: (since, bytes).
    today: HashMap<u32, (Instant, u64)>,
}

impl Files {
    pub fn new(stored: u64) -> Self {
        Files {
            stored,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------- uploads

pub(crate) async fn create(
    stream: &mut TcpStream,
    req: &Request,
    body: Vec<u8>,
    ip: &str,
    shared: &Shared,
) {
    let body = match body_size(req) {
        Some(BodySize::Known(len)) if len <= 4096 => match read_body(stream, body, len).await {
            Some(b) => b,
            None => return,
        },
        Some(BodySize::Chunked) => match read_chunked(stream, body, 4096).await {
            Ok(b) => b,
            Err(e) => {
                return error(
                    stream,
                    req,
                    400,
                    "bad_request",
                    &format!("Couldn't read the request ({e})."),
                )
                .await
            }
        },
        _ => {
            return error(
                stream,
                req,
                411,
                "bad_request",
                "Missing or too long request.",
            )
            .await
        }
    };
    let Ok(new) = serde_json::from_slice::<NewUpload>(&body) else {
        return error(stream, req, 400, "bad_request", "Unreadable request.").await;
    };
    let answer = start_upload(&mut shared.lock().unwrap(), req, &new, ip);
    match answer {
        Ok((id, dir)) => {
            let made = tokio::fs::create_dir_all(&dir).await.and(
                tokio::fs::File::create(dir.join(format!("{id}.part")))
                    .await
                    .map(|_| ()),
            );
            if let Err(e) = made {
                error!("files", "Couldn't save files in {} ({e})", dir.display());
                shared.lock().unwrap().files.uploads.remove(&id);
                return error(
                    stream,
                    req,
                    507,
                    "disk",
                    "The server couldn't save the file.",
                )
                .await;
            }
            json(stream, 200, &UploadCreated { id, chunk: CHUNK }).await;
        }
        Err((status, code, message)) => error(stream, req, status, code, &message).await,
    }
}

type Refusal = (u16, &'static str, String);

fn start_upload(
    st: &mut State,
    req: &Request,
    new: &NewUpload,
    ip: &str,
) -> Result<(String, PathBuf), Refusal> {
    let Some((conn, account)) = who(st, req) else {
        return Err((401, "signed_out", "Sign in again to send files.".into()));
    };
    let name = st.who(conn);
    let max = st.cfg.max_attachment_bytes;
    let now = Instant::now();
    let open = st
        .files
        .uploads
        .values()
        .filter(|u| u.owner == account && !u.done)
        .count();
    let recent = {
        let starts = st.files.starts.entry(account).or_default();
        starts.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        starts.len()
    };
    if new.size == 0 {
        return Err((400, "empty", "That file is empty.".into()));
    }
    if new.size > max {
        warn!(
            "files",
            "{name} tried to send a file that's too big ({})",
            files::size_label(new.size)
        );
        return Err((
            413,
            "too_big",
            format!(
                "That file is too big. The limit is {}.",
                files::size_label(max)
            ),
        ));
    }
    if new.size > st.cfg.max_storage_bytes {
        return Err((
            507,
            "too_big",
            "That file is bigger than all the space the server keeps for files.".into(),
        ));
    }
    let per_day = st.cfg.max_upload_bytes_per_day;
    let today = st.files.today.entry(account).or_insert((now, 0));
    if now.duration_since(today.0) > Duration::from_secs(24 * 3600) {
        *today = (now, 0);
    }
    if per_day > 0 && today.1 + new.size > per_day {
        warn!(
            "files",
            "{name} reached the daily limit for sending files ({})",
            files::size_label(per_day)
        );
        return Err((
            429,
            "daily_limit",
            format!(
                "You've sent {} of files today, the most this server allows in a day. Try again tomorrow.",
                files::size_label(today.1)
            ),
        ));
    }
    if recent >= UPLOADS_PER_MINUTE || open >= MAX_OPEN_UPLOADS {
        warn!("files", "{name} is sending files too fast");
        return Err((
            429,
            "slow_down",
            "You're sending files too fast. Wait a minute.".into(),
        ));
    }
    st.files.starts.entry(account).or_default().push(now);
    if let Some(t) = st.files.today.get_mut(&account) {
        t.1 += new.size;
    }
    let id = random_id();
    let clean = files::clean_name(&new.name);
    debug!(
        "files",
        "{name} is sending {clean} ({}) from {ip}",
        files::size_label(new.size)
    );
    st.files.uploads.insert(
        id.clone(),
        Upload {
            owner: account,
            name: clean,
            size: new.size,
            received: 0,
            head: Vec::new(),
            mime: "application/octet-stream",
            done: false,
            busy: false,
            created: now,
        },
    );
    Ok((id, attachments_dir(&st.cfg)))
}

enum Claim {
    Write(PathBuf),
    /// Not the next piece (or already done): the app carries on from here.
    Conflict(UploadProgress),
    Refuse(Refusal),
}

pub(crate) async fn receive(
    stream: &mut TcpStream,
    req: &Request,
    body: Vec<u8>,
    id: &str,
    shared: &Shared,
) {
    let offset: u64 = req
        .query_param("offset")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Strangers don't get to make the server read (and hold) a body.
    if who(&shared.lock().unwrap(), req).is_none() {
        return error(
            stream,
            req,
            401,
            "signed_out",
            "Sign in again to send files.",
        )
        .await;
    }
    // A piece either says its length (then it's written as it arrives) or comes
    // chunked through a proxy (then it's read whole first; pieces are small).
    let (len, body) = match body_size(req) {
        Some(BodySize::Known(n)) if n > 0 && n <= CHUNK => (n, body),
        Some(BodySize::Chunked) => match read_chunked(stream, body, CHUNK as usize).await {
            Ok(b) if !b.is_empty() => (b.len() as u64, b),
            Ok(_) => {
                return error(stream, req, 400, "bad_request", "An empty piece of a file.").await
            }
            Err(e) => {
                return error(
                    stream,
                    req,
                    400,
                    "bad_request",
                    &format!("Couldn't read a piece of the file ({e})."),
                )
                .await
            }
        },
        _ => {
            return error(
                stream,
                req,
                411,
                "bad_request",
                "Each piece must be 1 byte to 4 MB.",
            )
            .await
        }
    };
    let claim = claim_piece(&mut shared.lock().unwrap(), req, id, offset, len);
    let path = match claim {
        Claim::Write(p) => p,
        Claim::Conflict(p) => return json(stream, 409, &p).await,
        Claim::Refuse((status, code, message)) => {
            return error(stream, req, status, code, &message).await
        }
    };
    // Write the piece as it arrives, without holding the lock.
    let written = write_piece(stream, &path, offset, body, len).await;
    let answer = finish_piece(&mut shared.lock().unwrap(), id, &path, offset, len, written);
    match answer {
        Some(Ok(progress)) => json(stream, 200, &progress).await,
        Some(Err((status, code, message))) => error(stream, req, status, code, &message).await,
        None => {} // interrupted; the app retries from `received`
    }
}

fn claim_piece(st: &mut State, req: &Request, id: &str, offset: u64, len: u64) -> Claim {
    let account = who(st, req).map(|(_, a)| a);
    let dir = attachments_dir(&st.cfg);
    let Some(account) = account else {
        return Claim::Refuse((401, "signed_out", "Sign in again to send files.".into()));
    };
    match st.files.uploads.get_mut(id) {
        Some(u) if u.owner != account => {
            Claim::Refuse((403, "not_yours", "That upload isn't yours.".into()))
        }
        Some(u) if u.done || u.busy || offset != u.received || offset + len > u.size => {
            Claim::Conflict(UploadProgress {
                received: u.received,
                done: u.done,
            })
        }
        Some(u) => {
            u.busy = true;
            Claim::Write(dir.join(format!("{id}.part")))
        }
        None => Claim::Refuse((
            404,
            "gone",
            "That upload has expired. Send the file again.".into(),
        )),
    }
}

fn finish_piece(
    st: &mut State,
    id: &str,
    path: &std::path::Path,
    offset: u64,
    len: u64,
    written: Result<Vec<u8>, String>,
) -> Option<Result<UploadProgress, Refusal>> {
    let u = st.files.uploads.get_mut(id)?;
    u.busy = false;
    let head = match written {
        Ok(head) => head,
        Err(e) => {
            debug!("files", "Upload {id} interrupted: {e}");
            return None;
        }
    };
    if offset == 0 {
        u.head = head;
    }
    u.received = offset + len;
    if u.received < u.size {
        return Some(Ok(UploadProgress {
            received: u.received,
            done: false,
        }));
    }
    u.done = true;
    u.mime = files::sniff(&u.head);
    let size = u.size;
    if let Err(e) = std::fs::rename(path, path.with_extension("")) {
        error!("files", "Couldn't finish saving a file ({e})");
        st.files.uploads.remove(id);
        return Some(Err((
            507,
            "disk",
            "The server couldn't save the file.".into(),
        )));
    }
    st.files.stored += size;
    Some(Ok(UploadProgress {
        received: size,
        done: true,
    }))
}

/// Append `len` bytes (some already read) to the file. Returns its first bytes.
async fn write_piece(
    stream: &mut TcpStream,
    path: &PathBuf,
    offset: u64,
    mut start: Vec<u8>,
    len: u64,
) -> Result<Vec<u8>, String> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await
        .map_err(|e| e.to_string())?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| e.to_string())?;
    start.truncate(len as usize);
    let mut head: Vec<u8> = start.iter().take(64).copied().collect();
    file.write_all(&start).await.map_err(|e| e.to_string())?;
    let mut left = len - start.len() as u64;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = tokio::time::timeout(IDLE, stream.read(&mut buf[..want]))
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed".into());
        }
        if head.len() < 64 {
            let take = (64 - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        file.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
        left -= n as u64;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(head)
}

// ---------------------------------------------------------------- downloads

/// A file that's attached to a message in history (or is a poster of one).
fn find(st: &State, id: &str) -> Option<(String, String, u64)> {
    for a in st.history.values().flatten().flat_map(|m| &m.attachments) {
        if a.expired {
            continue;
        }
        if a.id == id {
            return Some((a.mime.clone(), a.name.clone(), a.size));
        }
        if let Some(p) = &a.poster {
            if p.id == id {
                return Some(("image/jpeg".into(), "poster.jpg".into(), 0));
            }
        }
    }
    st.archive
        .files
        .get(id)
        .map(|f| (f.mime.clone(), f.name.clone(), f.size))
}

pub(crate) async fn send(stream: &mut TcpStream, req: &Request, id: &str, shared: &Shared) {
    let found = {
        let st = shared.lock().unwrap();
        if who(&st, req).is_none() {
            None.ok_or((401, "signed_out"))
        } else if !valid_id(id) {
            None.ok_or((404, "gone"))
        } else {
            find(&st, id)
                .map(|f| (f, attachments_dir(&st.cfg).join(id)))
                .ok_or((404, "gone"))
        }
    };
    let ((mime, _name, _), path) = match found {
        Ok(f) => f,
        Err((401, code)) => return error(stream, req, 401, code, "Sign in again.").await,
        Err((status, code)) => {
            return error(
                stream,
                req,
                status,
                code,
                "This file is no longer available.",
            )
            .await
        }
    };
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => {
            return error(
                stream,
                req,
                404,
                "gone",
                "This file is no longer available.",
            )
            .await
        }
    };
    let total = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    // "Range: bytes=N-" (or N-M) resumes an interrupted download.
    let range = req
        .header("range")
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| {
            let (a, b) = r.split_once('-')?;
            let start: u64 = a.trim().parse().ok()?;
            let end: u64 = if b.trim().is_empty() {
                total.saturating_sub(1)
            } else {
                b.trim().parse().ok()?
            };
            Some((start, end.min(total.saturating_sub(1))))
        });
    let (status, start, end) = match range {
        Some((s, e)) if s <= e && s < total => (206, s, e),
        Some(_) => {
            let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(head.as_bytes()).await;
            return;
        }
        None => (200, 0, total.saturating_sub(1)),
    };
    let len = if total == 0 { 0 } else { end - start + 1 };
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nCache-Control: private, no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        if status == 206 { "Partial Content" } else { "OK" }
    );
    if status == 206 {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    }
    head.push_str("\r\n");
    if stream.write_all(head.as_bytes()).await.is_err() || len == 0 {
        return;
    }
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return;
    }
    let mut left = len;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = match file.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        match tokio::time::timeout(IDLE, stream.write_all(&buf[..n])).await {
            Ok(Ok(())) => left -= n as u64,
            _ => break,
        }
    }
    let _ = stream.flush().await;
}

// ---------------------------------------------------------------- posting

/// Attach finished uploads to a new chat message.
pub fn post(st: &mut State, conn: u32, channel: &str, text: &str, list: Vec<PostFile>) {
    let fail = |st: &State, message: &str| {
        st.send(
            conn,
            &ServerMsg::Error {
                code: "post_failed".into(),
                message: message.into(),
            },
        )
    };
    let Some(c) = st.clients.get(&conn) else {
        return;
    };
    let (Some(account), name) = (c.account, c.name.clone()) else {
        return;
    };
    if !st.cfg.text_channels.iter().any(|ch| ch == channel) {
        return fail(st, "That channel doesn't exist.");
    }
    if list.is_empty() || list.len() > MAX_FILES_PER_POST {
        return fail(
            st,
            &format!("Send 1 to {MAX_FILES_PER_POST} files at a time."),
        );
    }
    // Check everything before claiming anything.
    for f in &list {
        let ok = st
            .files
            .uploads
            .get(&f.upload)
            .is_some_and(|u| u.owner == account && u.done);
        let poster_ok = f.poster.as_ref().is_none_or(|p| {
            st.files.uploads.get(p).is_some_and(|u| {
                u.owner == account
                    && u.done
                    && u.size <= MAX_POSTER
                    && matches!(u.mime, "image/jpeg" | "image/png")
            })
        });
        if !ok || !poster_ok {
            return fail(st, "A file didn't finish uploading. Try sending it again.");
        }
    }
    let mut attachments = Vec::new();
    for f in list {
        let u = st.files.uploads.remove(&f.upload).unwrap();
        let kind = files::kind_of(u.mime);
        let visual = matches!(
            kind,
            files::Kind::Image | files::Kind::Gif | files::Kind::Video
        );
        // The poster is shown at the video's shape, so it borrows its size.
        let poster = f
            .poster
            .as_ref()
            .filter(|_| kind == files::Kind::Video)
            .and_then(|p| {
                st.files.uploads.remove(p).map(|_| Poster {
                    id: p.clone(),
                    width: f.width.min(20_000),
                    height: f.height.min(20_000),
                })
            });
        attachments.push(Attachment {
            id: f.upload.clone(),
            name: u.name,
            mime: u.mime.to_string(),
            size: u.size,
            width: if visual { f.width.min(20_000) } else { 0 },
            height: if visual { f.height.min(20_000) } else { 0 },
            duration_ms: if matches!(kind, files::Kind::Video | files::Kind::Audio) {
                f.duration_ms.min(24 * 3600 * 1000)
            } else {
                0
            },
            poster,
            expired: false,
        });
    }
    let text: String = text
        .replace("\r\n", "\n")
        .trim()
        .chars()
        .take(2000)
        .collect();
    info!(
        "chat",
        "{name} sent {} in #{channel}",
        if attachments.len() == 1 {
            format!("a file ({})", files::size_label(attachments[0].size))
        } else {
            format!("{} files", attachments.len())
        }
    );
    let message = ChatMessage {
        id: random_id(),
        channel: channel.to_string(),
        author: name,
        author_id: conn.to_string(),
        text,
        ts: now_ms(),
        attachments,
    };
    history::post(st, message);
    crate::storage::enforce(st);
}

/// Delete a stored file. Returns how many bytes that freed.
pub fn remove_file(dir: &std::path::Path, id: &str) -> u64 {
    let path = dir.join(id);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    match std::fs::remove_file(&path) {
        Ok(()) => size,
        Err(_) => 0,
    }
}

/// Forget uploads nobody attached to a message.
pub fn sweep(st: &mut State) {
    let dir = attachments_dir(&st.cfg);
    let stale: Vec<String> = st
        .files
        .uploads
        .iter()
        .filter(|(_, u)| !u.busy && u.created.elapsed() > UPLOAD_LIFETIME)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        let u = st.files.uploads.remove(&id).unwrap();
        if u.done {
            st.files.stored = st.files.stored.saturating_sub(remove_file(&dir, &id));
        } else {
            let _ = std::fs::remove_file(dir.join(format!("{id}.part")));
        }
        debug!("files", "Deleted an upload nobody posted ({})", u.name);
    }
}
