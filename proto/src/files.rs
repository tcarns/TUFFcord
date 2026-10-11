//! File attachments (0.7 and later): any kind of file, sent over plain HTTP on
//! the server's port instead of the WebSocket, so a big video never holds up
//! anyone's voice.
//!
//! 1. `POST /files` with `{name, size}` reserves an upload and returns its id.
//! 2. `PUT /files/<id>?offset=N` sends the file in pieces of up to [`CHUNK`] bytes,
//!    in order (so an interrupted upload can carry on where it stopped).
//! 3. A `Post` message on the WebSocket attaches finished uploads to a chat message.
//! 4. `GET /files/<id>` downloads one (`Range: bytes=N-` resumes).
//!
//! Every request carries `Authorization: Bearer <fileKey>`, the key from `Welcome`.

use serde::{Deserialize, Serialize};

/// Largest piece of an upload in one request.
pub const CHUNK: u64 = 4 * 1024 * 1024;
/// A poster (preview frame of a video) is a small JPEG.
pub const MAX_POSTER: u64 = 512 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewUpload {
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UploadCreated {
    pub id: String,
    pub chunk: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UploadProgress {
    pub received: u64,
    pub done: bool,
}

/// Body of an HTTP error answer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpError {
    pub code: String,
    pub message: String,
}

/// How the app shows an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    /// Animated (or not) GIF: shown like an image, but animated.
    Gif,
    Video,
    Audio,
    File,
}

pub fn kind_of(mime: &str) -> Kind {
    match mime {
        "image/gif" => Kind::Gif,
        m if crate::IMAGE_MIMES.contains(&m) => Kind::Image,
        m if m.starts_with("video/") => Kind::Video,
        m if m.starts_with("audio/") => Kind::Audio,
        _ => Kind::File,
    }
}

/// What a file really is, judged by its first bytes (never by its name), for
/// the types TUFFcord can show or play. Everything else is a plain file.
pub fn sniff(head: &[u8]) -> &'static str {
    if let Some(img) = crate::sniff_image(head) {
        return img;
    }
    let at = |i: usize, pat: &[u8]| head.len() >= i + pat.len() && &head[i..i + pat.len()] == pat;
    if at(4, b"ftyp") {
        // MP4 family: the brand says which.
        let brand = head.get(8..12).unwrap_or(b"");
        return match brand {
            b"M4A " | b"M4B " | b"M4P " => "audio/mp4",
            b"qt  " => "video/quicktime",
            b"heic" | b"heix" | b"mif1" | b"avif" => "application/octet-stream",
            _ => "video/mp4",
        };
    }
    if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        // Matroska / WebM. WebM says so in its DocType near the start.
        let webm = head.windows(4).take(64).any(|w| w == b"webm");
        return if webm {
            "video/webm"
        } else {
            "video/x-matroska"
        };
    }
    if at(0, b"RIFF") && at(8, b"WAVE") {
        return "audio/wav";
    }
    if at(0, b"RIFF") && at(8, b"AVI ") {
        return "video/x-msvideo";
    }
    if at(0, b"OggS") {
        return "audio/ogg";
    }
    if at(0, b"fLaC") {
        return "audio/flac";
    }
    if at(0, b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE6 == 0xE2) {
        // ID3 tag, or an MPEG audio frame sync (layer III).
        return "audio/mpeg";
    }
    if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xF6 == 0xF0 {
        return "audio/aac";
    }
    if at(0, b"%PDF-") {
        return "application/pdf";
    }
    "application/octet-stream"
}

/// The HTTP address of the server, from the WebSocket one:
/// `wss://x.trycloudflare.com/ws` -> `https://x.trycloudflare.com`.
pub fn http_base(ws_url: &str) -> String {
    let (scheme, rest) = match ws_url.split_once("://") {
        Some(("wss", r)) => ("https", r),
        Some(("ws", r)) => ("http", r),
        Some((s, r)) => (s, r),
        None => ("http", ws_url),
    };
    let host = rest.split('/').next().unwrap_or(rest);
    format!("{scheme}://{host}")
}

/// "240 KB", "1.4 MB", "1.2 GB".
pub fn size_label(bytes: u64) -> String {
    const MB: f64 = 1_048_576.0;
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (MB * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    }
}

/// "0:07", "3:25", "1:02:03".
pub fn duration_label(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A file name that's safe to show and to save under on any system.
pub fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            // Bidi controls too: "photo\u{202E}gpj.exe" would show as "photoexe.jpg".
            if c.is_control()
                || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
                || matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    // Keep the end (where the extension is) if it's long.
    let n = cleaned.chars().count();
    let cleaned: String = if n > 100 {
        cleaned.chars().skip(n - 100).collect()
    } else {
        cleaned.to_string()
    };
    if cleaned.is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

/// The extension of a file name, lowercase, without the dot ("" if none).
pub fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && ext.len() <= 8
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            ext.to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

/// Extensions Windows runs as programs or scripts when opened.
pub fn runs_code(name: &str) -> bool {
    matches!(
        extension(name).as_str(),
        "exe"
            | "com"
            | "bat"
            | "cmd"
            | "msi"
            | "msix"
            | "appx"
            | "ps1"
            | "psm1"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "hta"
            | "scr"
            | "pif"
            | "lnk"
            | "reg"
            | "cpl"
            | "jar"
            | "dll"
            | "sys"
            | "inf"
            | "application"
            | "msc"
            | "url"
            | "msp"
            | "scf"
            | "chm"
            | "iso"
            | "img"
            | "vhd"
            | "vhdx"
            | "xll"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        let mp4 = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41";
        assert_eq!(sniff(mp4), "video/mp4");
        assert_eq!(sniff(b"\0\0\0\x14ftypqt  \0\0\0\0"), "video/quicktime");
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A \0\0\0\0"), "audio/mp4");
        let mut webm = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01];
        webm.extend_from_slice(b"\x42\x82\x84webm");
        assert_eq!(sniff(&webm), "video/webm");
        assert_eq!(
            sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, b'm', b'a', b't']),
            "video/x-matroska"
        );
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), "audio/wav");
        assert_eq!(sniff(b"OggS\0\x02"), "audio/ogg");
        assert_eq!(sniff(b"ID3\x04\0"), "audio/mpeg");
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x44]), "audio/mpeg");
        assert_eq!(sniff(b"fLaC\0"), "audio/flac");
        assert_eq!(sniff(b"%PDF-1.7"), "application/pdf");
        assert_eq!(sniff(b"GIF89a.."), "image/gif");
        assert_eq!(sniff(b"MZ\x90\0"), "application/octet-stream");
        assert_eq!(sniff(b""), "application/octet-stream");
        assert_eq!(kind_of("image/gif"), Kind::Gif);
        assert_eq!(kind_of("image/png"), Kind::Image);
        assert_eq!(kind_of("video/webm"), Kind::Video);
        assert_eq!(kind_of("audio/ogg"), Kind::Audio);
        assert_eq!(kind_of("application/pdf"), Kind::File);
        // A name doesn't make something an image.
        assert_eq!(kind_of("image/svg+xml"), Kind::File);
    }

    #[test]
    fn names_and_labels() {
        assert_eq!(
            http_base("wss://a.trycloudflare.com/ws"),
            "https://a.trycloudflare.com"
        );
        assert_eq!(http_base("ws://localhost:3000/ws"), "http://localhost:3000");
        assert_eq!(clean_name("C:\\Users\\me\\clip.mp4"), "clip.mp4");
        assert_eq!(clean_name("a<b>:c?.txt"), "a_b__c_.txt");
        assert_eq!(clean_name("  ...  "), "file");
        assert_eq!(clean_name("photo\u{202E}gpj.exe"), "photo_gpj.exe");
        assert!(runs_code("photo\u{202E}gpj.exe") && runs_code("disk.ISO"));
        assert!(clean_name(&format!("{}.mp4", "x".repeat(300))).ends_with("x.mp4"));
        assert_eq!(extension("Clip.MP4"), "mp4");
        assert_eq!(extension("archive.tar.gz"), "gz");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("noext"), "");
        assert!(runs_code("setup.EXE"));
        assert!(runs_code("x.ps1"));
        assert!(!runs_code("notes.txt"));
        assert_eq!(size_label(1), "1 KB");
        assert_eq!(size_label(1_572_864), "1.5 MB");
        assert_eq!(size_label(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(duration_label(7_400), "0:07");
        assert_eq!(duration_label(205_000), "3:25");
        assert_eq!(duration_label(3_723_000), "1:02:03");
    }
}
