//! Plain HTTP on the server's port: reading requests, answering them, and
//! sending each to its handler (files in `files`, profile pictures in `avatars`).

use crate::{avatars, files, Shared, State};
use proto::files::HttpError;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Waiting longer than this for the next bytes ends the request.
pub(crate) const IDLE: Duration = Duration::from_secs(60);

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

    pub(crate) fn query_param(&self, key: &str) -> Option<&str> {
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

pub(crate) async fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
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

pub(crate) async fn json<T: serde::Serialize>(stream: &mut TcpStream, status: u16, value: &T) {
    let body = serde_json::to_vec(value).unwrap_or_default();
    respond(stream, status, "application/json", &body).await;
}

/// Refuse a request, and say so in the log (with who asked) so problems
/// sending or fetching files can be traced from the server window.
pub(crate) async fn error(
    stream: &mut TcpStream,
    req: &Request,
    status: u16,
    code: &str,
    message: &str,
) {
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
pub(crate) fn who(st: &State, req: &Request) -> Option<(u32, u32)> {
    let key = req.header("authorization")?.strip_prefix("Bearer ")?.trim();
    let conn = *st.files.keys.get(key)?;
    let c = st.clients.get(&conn)?;
    if !c.authed {
        return None;
    }
    Some((conn, c.account?))
}

pub(crate) fn valid_id(id: &str) -> bool {
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
        ("POST", "/files") => files::create(&mut stream, &req, body_start, &ip, &shared).await,
        ("PUT", p) if p.starts_with("/files/") => {
            files::receive(&mut stream, &req, body_start, &p[7..], &shared).await
        }
        ("PUT" | "DELETE", "/avatar") => avatars::set(&mut stream, &req, body_start, &shared).await,
        ("GET", p) if p.starts_with("/avatars/") => {
            avatars::get(&mut stream, &req, &p[9..], &shared).await
        }
        ("GET", p) if p.starts_with("/files/") => {
            files::send(&mut stream, &req, &p[7..], &shared).await
        }
        _ => {
            debug!(
                "http",
                "{ip} asked for {} {} (not found)", req.method, req.path
            );
            error(&mut stream, &req, 404, "not_found", "Nothing here.").await;
        }
    }
}

pub(crate) enum BodySize {
    Known(u64),
    /// `Transfer-Encoding: chunked`, which is how Cloudflare's tunnel passes
    /// request bodies on by default.
    Chunked,
}

pub(crate) fn body_size(req: &Request) -> Option<BodySize> {
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
pub(crate) async fn fill<R: tokio::io::AsyncRead + Unpin>(
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
pub(crate) async fn take_line<R: tokio::io::AsyncRead + Unpin>(
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
pub(crate) async fn read_chunked<R: tokio::io::AsyncRead + Unpin>(
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
pub(crate) async fn read_body(
    stream: &mut TcpStream,
    mut start: Vec<u8>,
    len: u64,
) -> Option<Vec<u8>> {
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
