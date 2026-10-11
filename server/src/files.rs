//! Files over plain HTTP on the server's port (see `proto::files`), so a big
//! video never holds up anyone's voice on the WebSocket.
//!
//! Uploads are written to `data/attachments/<id>.part` as they arrive and renamed
//! to `<id>` when complete. A finished upload belongs to whoever sent it until a
//! `Post` message attaches it to a chat message; unclaimed ones are deleted after
//! half an hour. What a file is (image, video…) is decided here from its first
//! bytes, never from the sender's word.

use crate::{attachments_dir, history, now_ms, random_id, Shared, State};
use proto::files::{self, HttpError, NewUpload, UploadCreated, UploadProgress, CHUNK, MAX_POSTER};
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
/// Waiting longer than this for the next bytes ends the request.
const IDLE: Duration = Duration::from_secs(60);
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

// ---------------------------------------------------------------- the request head

pub struct Request {
    /// Who's asking (filled in once the real address is known), for the log.
    pub ip: String,
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_websocket(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    }

    fn query_param(&self, key: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }
}

/// Read up to the end of the request head. Returns the parsed head and any
/// bytes after it (the start of a body).
pub async fn read_head(stream: &mut TcpStream) -> Result<(Request, Vec<u8>), String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut chunk))
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut headers = [httparse::EMPTY_HEADER; 48];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&buf) {
            Ok(httparse::Status::Complete(len)) => {
                let target = req.path.unwrap_or("/");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let parsed = Request {
                    ip: String::new(),
                    method: req.method.unwrap_or("").to_string(),
                    path: path.to_string(),
                    query: query.to_string(),
                    headers: req
                        .headers
                        .iter()
                        .map(|h| {
                            (
                                h.name.to_string(),
                                String::from_utf8_lossy(h.value).trim().to_string(),
                            )
                        })
                        .collect(),
                };
                return Ok((parsed, buf[len..].to_vec()));
            }
            Ok(httparse::Status::Partial) if buf.len() < 16 * 1024 => continue,
            Ok(httparse::Status::Partial) => return Err("request head too large".into()),
            Err(e) => return Err(format!("not HTTP ({e})")),
        }
    }
}

// ---------------------------------------------------------------- answers

async fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        429 => "Too Many Requests",
        507 => "Insufficient Storage",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.flush().await;
}

async fn json<T: serde::Serialize>(stream: &mut TcpStream, status: u16, value: &T) {
    let body = serde_json::to_vec(value).unwrap_or_default();
    respond(stream, status, "application/json", &body).await;
}

/// Refuse a request, and say so in the log (with who asked) so problems
/// sending or fetching files can be traced from the server window.
async fn error(stream: &mut TcpStream, req: &Request, status: u16, code: &str, message: &str) {
    let what = format!(
        "Refused {} {} from {} ({status} {code}): {message}",
        req.method, req.path, req.ip
    );
    match status {
        // The app's own mistakes or a proxy changing requests: worth noticing.
        400 | 411 | 413 | 507 => warn!("files", "{what}"),
        _ => info!("files", "{what}"),
    }
    json(
        stream,
        status,
        &HttpError {
            code: code.into(),
            message: message.into(),
        },
    )
    .await;
}

// ---------------------------------------------------------------- routing

/// Who's asking: (connection, account), if their file key is good.
fn who(st: &State, req: &Request) -> Option<(u32, u32)> {
    let key = req.header("authorization")?.strip_prefix("Bearer ")?.trim();
    let conn = *st.files.keys.get(key)?;
    let c = st.clients.get(&conn)?;
    if !c.authed {
        return None;
    }
    Some((conn, c.account?))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// A plain HTTP request (anything but the WebSocket).
pub async fn handle(
    mut stream: TcpStream,
    mut req: Request,
    body_start: Vec<u8>,
    ip: String,
    shared: Shared,
) {
    req.ip = ip.clone();
    debug!("http", "{ip}: {} {}", req.method, req.path);
    let path = req.path.trim_end_matches('/');
    match (req.method.as_str(), path) {
        ("GET" | "HEAD", "") => {
            let text = format!("TUFFcord server {}\n", proto::update::CURRENT);
            respond(
                &mut stream,
                200,
                "text/plain; charset=utf-8",
                text.as_bytes(),
            )
            .await;
        }
        ("POST", "/files") => create(&mut stream, &req, body_start, &ip, &shared).await,
        ("PUT", p) if p.starts_with("/files/") => {
            receive(&mut stream, &req, body_start, &p[7..], &shared).await
        }
        ("GET", p) if p.starts_with("/files/") => send(&mut stream, &req, &p[7..], &shared).await,
        _ => {
            debug!(
                "http",
                "{ip} asked for {} {} (not found)", req.method, req.path
            );
            error(&mut stream, &req, 404, "not_found", "Nothing here.").await;
        }
    }
}

enum BodySize {
    Known(u64),
    /// `Transfer-Encoding: chunked`, which is how Cloudflare's tunnel passes
    /// request bodies on by default.
    Chunked,
}

fn body_size(req: &Request) -> Option<BodySize> {
    if req
        .header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        return Some(BodySize::Chunked);
    }
    req.header("content-length")?
        .parse()
        .ok()
        .map(BodySize::Known)
}

/// Read more bytes into `buf` until it holds at least `n`.
async fn fill<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
    n: usize,
) -> Result<(), String> {
    let mut chunk = [0u8; 16 * 1024];
    while buf.len() < n {
        let got = tokio::time::timeout(IDLE, r.read(&mut chunk))
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if got == 0 {
            return Err("the connection closed".into());
        }
        buf.extend_from_slice(&chunk[..got]);
    }
    Ok(())
}

/// Take one CRLF-terminated line off the front of `buf`.
async fn take_line<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
) -> Result<String, String> {
    loop {
        if let Some(i) = buf.windows(2).position(|w| w == b"\r\n") {
            let line = String::from_utf8_lossy(&buf[..i]).to_string();
            buf.drain(..i + 2);
            return Ok(line);
        }
        if buf.len() > 8 * 1024 {
            return Err("a line in the request was too long".into());
        }
        let want = buf.len() + 1;
        fill(r, buf, want).await?;
    }
}

/// Read a whole chunked body (up to `max` bytes). `buf` holds what's already read.
async fn read_chunked<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    mut buf: Vec<u8>,
    max: usize,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line = take_line(r, &mut buf).await?;
        let hex = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(hex, 16).map_err(|_| format!("bad chunk size {hex:?}"))?;
        if size == 0 {
            // Optional trailers, then a blank line.
            while !take_line(r, &mut buf).await?.is_empty() {}
            return Ok(out);
        }
        if out.len() + size > max {
            return Err(format!("the body is over {max} bytes"));
        }
        fill(r, &mut buf, size + 2).await?;
        if &buf[size..size + 2] != b"\r\n" {
            return Err("a chunk didn't end where it said".into());
        }
        out.extend_from_slice(&buf[..size]);
        buf.drain(..size + 2);
    }
}

/// Read exactly `len` body bytes (starting with what came with the head).
async fn read_body(stream: &mut TcpStream, mut start: Vec<u8>, len: u64) -> Option<Vec<u8>> {
    start.truncate(len as usize);
    let mut rest = vec![0u8; (len as usize).saturating_sub(start.len())];
    if !rest.is_empty() {
        tokio::time::timeout(IDLE, stream.read_exact(&mut rest))
            .await
            .ok()?
            .ok()?;
    }
    start.extend_from_slice(&rest);
    Some(start)
}

// ---------------------------------------------------------------- uploads

async fn create(stream: &mut TcpStream, req: &Request, body: Vec<u8>, ip: &str, shared: &Shared) {
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

async fn receive(stream: &mut TcpStream, req: &Request, body: Vec<u8>, id: &str, shared: &Shared) {
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

async fn send(stream: &mut TcpStream, req: &Request, id: &str, shared: &Shared) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn chunked_bodies() {
        // Some already read with the head, the rest still to come.
        let mut rest: &[u8] = b"lo\r\n6;ext=1\r\n world\r\n0\r\nX-Trailer: 1\r\n\r\nNEXT";
        let got = read_chunked(&mut rest, b"5\r\nhel".to_vec(), 100)
            .await
            .unwrap();
        assert_eq!(got, b"hello world");
        let mut r: &[u8] = b"";
        assert!(
            read_chunked(&mut r, b"5\r\nhello\r\n0\r\n\r\n".to_vec(), 4)
                .await
                .is_err(),
            "over the limit"
        );
        let mut r: &[u8] = b"";
        assert!(read_chunked(&mut r, b"zz\r\n".to_vec(), 100).await.is_err());
        let mut r: &[u8] = b"";
        assert!(
            read_chunked(&mut r, b"5\r\nhel".to_vec(), 100)
                .await
                .is_err(),
            "cut off"
        );
        let mut r: &[u8] = b"";
        assert_eq!(
            read_chunked(&mut r, b"0\r\n\r\n".to_vec(), 100)
                .await
                .unwrap(),
            b""
        );
    }

    #[test]
    fn ids() {
        assert!(valid_id("0123abcdef"));
        assert!(!valid_id("../etc/passwd"));
        assert!(!valid_id(""));
        assert!(!valid_id("abc.part"));
    }
}
