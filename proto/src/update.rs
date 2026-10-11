//! Checks GitHub for a newer published release of TUFFcord, and installs one.
//!
//! Releases are created by the GitHub Actions workflow when the version in
//! Cargo.toml goes up. Installing (used by both the app and the server):
//!
//! 1. Download the new program next to the running one (`<name>.download`).
//! 2. Check its SHA-256 against the release's `SHA256SUMS.txt`. A mismatch stops here.
//! 3. Swap: rename the running program to `<name>.old` (Windows allows renaming a
//!    running program, just not overwriting or deleting it), then move the new file
//!    into place. If that second step fails, the old file is put back.
//! 4. The caller restarts into the new program, which deletes the leftovers.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The GitHub repository releases are published to.
pub const REPO: &str = "tcarns/TUFFcord";
/// Its name before the rename (copies up to 0.7.1 look there).
const OLD_REPO: &str = "tcarns/backroom";
/// The running program's version (from Cargo.toml).
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// How often to look for updates while running.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Without the leading "v", e.g. "0.3.0".
    pub version: String,
    /// The release page, where the download is.
    pub url: String,
    /// What changed, as written in the release notes (trimmed).
    pub notes: String,
    /// Files attached to the release (the zip, the raw .exe files, SHA256SUMS.txt).
    pub assets: Vec<Asset>,
    /// Its checksum file, once the signature has been checked ([`check`] does).
    pub sums: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    /// Direct download link.
    pub url: String,
    pub size: u64,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }
}

/// An HTTPS client using the operating system's TLS (SChannel on Windows).
pub fn agent(timeout: Duration) -> ureq::Agent {
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
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into()
}

/// The checksum file every release carries. Its first line is `version X.Y.Z`
/// (from 0.11.1), so a signed file can't be passed off as another release.
pub const SUMS_FILE: &str = "SHA256SUMS.txt";
/// Its signature, made by the owner on his own PC with `ssh-keygen -Y sign`
/// (`tools/sign-release.ps1`); the key never goes near GitHub. A checksum only
/// proves a download is whole; the signature proves the release came from us.
pub const SIG_FILE: &str = "SHA256SUMS.txt.sshsig";
/// What copies up to 0.11.0 check instead (a raw Ed25519 signature made by
/// the release workflow with an older key).
pub const OLD_SIG_FILE: &str = "SHA256SUMS.txt.sig";
/// `ssh-keygen -Y sign -n`: a release signature can't be reused for anything else.
pub const SIG_NAMESPACE: &str = "tuffcord-release";
/// The public half of the owner's signing key (`tuffcord-sign.pub`).
const RELEASE_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGTmSrl747wqhkZKo97lxUOfKbbAg884w2KPc3AvdVp4 tuffcord-sign";

/// The key releases must be signed with. Tests that serve their own releases
/// (`BACKROOM_UPDATE_URL`) may bring their own key.
fn release_key() -> String {
    match (
        std::env::var("BACKROOM_UPDATE_URL"),
        std::env::var("BACKROOM_UPDATE_KEY"),
    ) {
        (Ok(_), Ok(k)) => k,
        _ => RELEASE_KEY.to_string(),
    }
}

/// The version a checksum file says it belongs to.
pub fn sums_version(sums: &str) -> Option<&str> {
    sums.lines()
        .find_map(|l| l.trim().strip_prefix("version "))
        .map(str::trim)
}

/// True if `sig` is the release key's signature of `sums`, and `sums` says it
/// belongs to `version`.
pub fn signed_for(sums: &str, sig: &str, version: &str) -> bool {
    signed_with(&release_key(), sums, sig, version)
}

fn signed_with(key: &str, sums: &str, sig: &str, version: &str) -> bool {
    crate::sshsig::verify(key, SIG_NAMESPACE, sums.as_bytes(), sig)
        && sums_version(sums).is_some_and(|v| v.trim_start_matches(['v', 'V']) == version)
}

/// Fetch `release`'s checksums and signature and check them. Returns the
/// checksums, or why the release can't be trusted.
fn verified_sums(agent: &ureq::Agent, release: &Release, current: &str) -> Result<String, String> {
    let fetch = |name: &str, limit: u64| -> Result<String, String> {
        let asset = release.asset(name).ok_or("missing")?;
        let mut resp = agent
            .get(&asset.url)
            .header("User-Agent", &format!("TUFFcord/{current}"))
            .call()
            .map_err(|e| format!("couldn't reach GitHub ({e})"))?;
        if resp.status().as_u16() != 200 {
            return Err(format!(
                "GitHub answered with an error ({})",
                resp.status().as_u16()
            ));
        }
        resp.body_mut()
            .with_config()
            .limit(limit)
            .read_to_string()
            .map_err(|e| format!("couldn't read {name} ({e})"))
    };
    let unsigned = || {
        format!(
            "TUFFcord {} is out, but it isn't signed with TUFFcord's release key, so it was ignored.",
            release.version
        )
    };
    let (sums, sig) = match (fetch(SUMS_FILE, 64 * 1024), fetch(SIG_FILE, 4096)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) if e != "missing" => return Err(e),
        _ => return Err(unsigned()),
    };
    if !signed_for(&sums, &sig, &release.version) {
        return Err(unsigned());
    }
    Ok(sums)
}
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// "v1.2.3", "1.2", "1.2.3-beta" -> (1, 2, 3)
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Ask GitHub for the newest release. `Ok(None)` means you're up to date
/// (or nothing has been published yet).
pub fn check() -> Result<Option<Release>, String> {
    check_against(CURRENT)
}

pub fn check_against(current: &str) -> Result<Option<Release>, String> {
    // BACKROOM_UPDATE_URL lets tests point this at a local file server.
    let urls = match std::env::var("BACKROOM_UPDATE_URL") {
        Ok(u) => vec![u],
        // The repository was called backroom before. GitHub sends that name on to
        // the new one, and until the rename the new name isn't found yet.
        Err(_) => [REPO, OLD_REPO]
            .iter()
            .map(|r| format!("https://api.github.com/repos/{r}/releases/latest"))
            .collect(),
    };
    let agent = agent(Duration::from_secs(15));
    let mut found = None;
    for url in &urls {
        let resp = agent
            .get(url)
            .header("User-Agent", &format!("TUFFcord/{current}"))
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| format!("couldn't reach GitHub ({e})"))?;
        match resp.status().as_u16() {
            200 => {
                found = Some(resp);
                break;
            }
            404 => continue, // no releases yet (or nothing under this name)
            403 | 429 => return Err("GitHub asked us to slow down; will try again later".into()),
            code => return Err(format!("GitHub answered with an error ({code})")),
        }
    }
    let Some(mut resp) = found else {
        return Ok(None);
    };
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("couldn't read GitHub's answer ({e})"))?;
    let Some(mut release) = parse_release(&body, current)? else {
        return Ok(None);
    };
    // Only a release signed with our key is offered at all, so a stranger who
    // got into the GitHub account can't put a "Download" button in every app.
    release.sums = verified_sums(&agent, &release, current)?;
    Ok(Some(release))
}

pub fn parse_release(json: &str, current: &str) -> Result<Option<Release>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected answer from GitHub ({e})"))?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = v["tag_name"]
        .as_str()
        .ok_or("unexpected answer from GitHub (no version)")?;
    if !is_newer(tag, current) {
        return Ok(None);
    }
    let notes: String = v["body"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(600)
        .collect();
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: v["html_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(releases_page),
        notes,
        assets: v["assets"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|a| {
                        Some(Asset {
                            name: a["name"].as_str()?.to_string(),
                            url: a["browser_download_url"].as_str()?.to_string(),
                            size: a["size"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        sums: String::new(),
    }))
}

/// True if `release` has `asset` and its signature checked out, so it can be
/// installed automatically.
pub fn installable(release: &Release, asset: &str) -> bool {
    release.asset(asset).is_some() && !release.sums.is_empty()
}

/// Download `asset` from `release` next to `exe` and check it against the
/// release's signed checksums (fetched and checked by [`check`]). Returns the verified file (`<exe>.download`), ready for
/// [`install`]. `progress` gets (bytes so far, total) every so often.
pub fn download_verified(
    release: &Release,
    asset: &str,
    exe: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let file_asset = release
        .asset(asset)
        .ok_or_else(|| format!("This release doesn't include {asset}."))?;
    if release.sums.is_empty() {
        return Err("This release isn't signed, so it can't be installed safely.".into());
    }
    let dir = exe
        .parent()
        .ok_or("Couldn't find the program's folder.")?
        .to_path_buf();
    let agent = agent(Duration::from_secs(600));
    let user_agent = format!("TUFFcord/{CURRENT}");
    let sums_text = &release.sums;
    let expected = expected_hash(sums_text, asset)
        .ok_or_else(|| format!("The checksum file doesn't list {asset}."))?;

    // Download next to the running program, so the final move stays on the same drive.
    let tmp = suffixed(exe, ".download");
    let mut file = File::create(&tmp).map_err(|e| {
        format!(
            "TUFFcord can't save files in its folder ({}): {e}. Move it to a folder you own, like Documents\\TUFFcord, or update manually.",
            dir.display()
        )
    })?;
    let fail = |file: File, msg: String| {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        Err(msg)
    };
    let mut resp = match agent
        .get(&file_asset.url)
        .header("User-Agent", &user_agent)
        .call()
    {
        Ok(r) => r,
        Err(e) => return fail(file, format!("The download failed ({e}).")),
    };
    if resp.status().as_u16() != 200 {
        let code = resp.status().as_u16();
        return fail(
            file,
            format!("The download failed (GitHub answered {code})."),
        );
    }
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(file_asset.size);
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_report = Instant::now();
    progress(0, total);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                return fail(
                    file,
                    format!("The download was interrupted ({e}). Try again."),
                )
            }
        };
        hasher.update(&buf[..n]);
        if let Err(e) = file.write_all(&buf[..n]) {
            return fail(file, format!("Couldn't save the download ({e})."));
        }
        done += n as u64;
        if last_report.elapsed() > Duration::from_millis(100) {
            progress(done, total);
            last_report = Instant::now();
        }
    }
    progress(done, total);
    let _ = file.sync_all();
    drop(file);

    // Verify before touching the installed program.
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !got.eq_ignore_ascii_case(&expected) {
        let _ = std::fs::remove_file(&tmp);
        return Err("The download didn't match its checksum (it may be incomplete). Nothing was changed. Try again.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    Ok(tmp)
}

/// Program names before the rename and after.
const OLD_NAMES: [(&str, &str); 2] = [
    ("backroom", "TUFFcord"),
    ("backroom-server", "TUFFcord-server"),
];

static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The running program's path: `std::env::current_exe`, except after
/// [`adopt_new_name`] renamed it (Windows keeps reporting the old path then).
pub fn current_exe() -> std::io::Result<PathBuf> {
    match EXE.get() {
        Some(p) => Ok(p.clone()),
        None => std::env::current_exe(),
    }
}

/// Copies from before 0.8 update themselves in place, so they keep running as
/// `backroom.exe` / `backroom-server.exe`. Rename the running program to its
/// TUFFcord name (Windows allows that while it runs), unless something by that
/// name is already there. Returns the old and new paths when it renamed.
pub fn adopt_new_name() -> Option<(PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let new = new_name(&exe)?;
    if new.exists() {
        return None;
    }
    std::fs::rename(&exe, &new).ok()?;
    // The previous copy may still be closing: give its leftovers the new name
    // too, so `cleanup_leftovers` finds them.
    let old_len = exe.file_name().map_or(0, |n| n.len());
    for p in old_copies(&exe) {
        if let Some(suffix) = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.get(old_len..))
        {
            let _ = std::fs::rename(&p, suffixed(&new, suffix));
        }
    }
    let _ = std::fs::remove_file(suffixed(&exe, ".download"));
    let _ = EXE.set(new.clone());
    Some((exe, new))
}

/// `…/backroom.exe` → `…/TUFFcord.exe` (any case, with or without `.exe`).
fn new_name(exe: &Path) -> Option<PathBuf> {
    let name = exe.file_name()?.to_str()?;
    let stem = match name.len().checked_sub(4) {
        Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") => &name[..i],
        _ => name,
    };
    let (_, new) = OLD_NAMES
        .iter()
        .find(|(old, _)| old.eq_ignore_ascii_case(stem))?;
    Some(exe.with_file_name(format!("{new}{}", &name[stem.len()..])))
}

/// Put `new_file` where `exe` is, keeping the old one as `<exe>.old` until it
/// can be deleted. If an earlier `.old` is still running (the server's watcher
/// keeps the first copy open), the next free `.old1`, `.old2`… is used.
pub fn install(new_file: &Path, exe: &Path) -> std::io::Result<()> {
    remove_old_copies(exe);
    let old = (0..100)
        .map(|n| {
            suffixed(
                exe,
                &if n == 0 {
                    ".old".into()
                } else {
                    format!(".old{n}")
                },
            )
        })
        .find(|p| !p.exists())
        .ok_or_else(|| std::io::Error::other("too many old copies are still running"))?;
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new_file, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// `<exe><suffix>`, e.g. `backroom.exe.old`.
pub fn suffixed(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    exe.with_file_name(name)
}

/// The `<exe>.old`, `<exe>.old1`… copies left by earlier updates.
fn old_copies(exe: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let prefix = format!("{}.old", name.to_lowercase());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.strip_prefix(&prefix)
                .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|e| e.path())
        .collect()
}

/// Delete old copies that aren't running any more. True if none are left.
pub fn remove_old_copies(exe: &Path) -> bool {
    let mut all = true;
    for p in old_copies(exe) {
        if std::fs::remove_file(&p).is_err() {
            all = false;
        }
    }
    all
}

/// Remove what an update left behind. The previous copy may still be closing,
/// so keep trying for a few seconds (in the background).
pub fn cleanup_leftovers() {
    let Ok(exe) = current_exe() else {
        return;
    };
    let download = suffixed(&exe, ".download");
    if old_copies(&exe).is_empty() && !download.exists() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("cleanup".into())
        .spawn(move || {
            let _ = std::fs::remove_file(&download);
            for _ in 0..20 {
                if remove_old_copies(&exe) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
}

/// Find `name`'s hash in a `sha256sum`-style file ("<hex>  name" or "<hex> *name").
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file.eq_ignore_ascii_case(name)
            && hash.len() == 64
            && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_name_their_version() {
        // A test key and signature (ssh-keygen -Y sign -n tuffcord-release).
        let key =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHJId6PrhlnyAWbYEcm1FG7SMokqQauHNvmWGen8GUwT test";
        let sums = "version 1.2.3\nabc  TUFFcord.exe\n";
        let sig = "-----BEGIN SSH SIGNATURE-----
U1NIU0lHAAAAAQAAADMAAAALc3NoLWVkMjU1MTkAAAAgckh3o+uGWfIBZtgRybUUbtIyiS
pBq4c2+ZYZ6fwZTBMAAAAQdHVmZmNvcmQtcmVsZWFzZQAAAAAAAAAGc2hhNTEyAAAAUwAA
AAtzc2gtZWQyNTUxOQAAAECWQxNlhxfAIxJVsOxHZGtrvKeWlFWE0007sKxwmUd4fihIfa
knCwr3WUNWQFvwPM5Y2wFCkuFCmwQCsNYSBUkJ
-----END SSH SIGNATURE-----";
        assert!(signed_with(key, sums, sig, "1.2.3"));
        assert!(
            !signed_with(key, sums, sig, "1.3.0"),
            "an older release passed off as a newer one"
        );
        assert_eq!(sums_version(sums), Some("1.2.3"));
        assert_eq!(sums_version("abc  x.exe\n"), None);
        // Copies up to 0.11.0 skip the version line when looking up a hash.
        assert!(expected_hash(sums, "version").is_none());
    }

    #[test]
    fn finds_hashes() {
        let sums = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  backroom.exe\nFEDCBA9876543210FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210 *backroom-server.exe\n";
        assert_eq!(
            expected_hash(sums, "backroom.exe").unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(
            expected_hash(sums, "backroom-server.exe").unwrap(),
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
        );
        assert!(expected_hash(sums, "other.exe").is_none());
        assert!(expected_hash("nothex  backroom.exe", "backroom.exe").is_none());
    }

    #[test]
    fn install_swaps_and_keeps_old() {
        let dir = std::env::temp_dir().join(format!("br-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("backroom.exe");
        let new = dir.join("backroom.exe.download");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        install(&new, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join("backroom.exe.old")).unwrap(), b"old");
        assert!(!new.exists());
        // A failed swap puts the old file back.
        let missing = dir.join("nope.download");
        assert!(install(&missing, &exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        // Unrelated files that merely start with the name are left alone.
        std::fs::write(dir.join("backroom.exe.oldnotes.txt"), b"x").unwrap();
        assert!(remove_old_copies(&exe));
        assert!(dir.join("backroom.exe.oldnotes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn install_skips_old_copies_that_cant_be_removed() {
        // Stand-in for Windows refusing to delete a program that's still running:
        // a directory named like an old copy can't be removed with remove_file.
        let dir = std::env::temp_dir().join(format!("br-install-busy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("srv.exe.old")).unwrap();
        let exe = dir.join("srv.exe");
        std::fs::write(&exe, b"v2").unwrap();
        std::fs::write(dir.join("srv.exe.download"), b"v3").unwrap();
        install(&dir.join("srv.exe.download"), &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"v3");
        assert_eq!(std::fs::read(dir.join("srv.exe.old1")).unwrap(), b"v2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.3"), Some((0, 3, 0)));
        assert_eq!(parse_version("2.0.1-beta.1"), Some((2, 0, 1)));
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("nonsense", "0.2.0"));
    }

    #[test]
    fn release_assets() {
        let json = r#"{"tag_name":"v0.4.0","html_url":"h","body":"","assets":[
            {"name":"backroom.exe","browser_download_url":"https://github.com/tcarns/backroom/releases/download/v0.4.0/backroom.exe","size":9530368},
            {"name":"SHA256SUMS.txt","browser_download_url":"https://x/SHA256SUMS.txt","size":300}]}"#;
        let r = parse_release(json, "0.3.0").unwrap().unwrap();
        assert_eq!(r.asset("Backroom.exe").unwrap().size, 9530368);
        assert!(r.asset("SHA256SUMS.txt").is_some());
        assert!(r.asset("missing.exe").is_none());
    }

    #[test]
    fn release_json() {
        let json = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/tcarns/backroom/releases/tag/v0.3.0","body":"Screen sharing","draft":false,"prerelease":false}"#;
        let r = parse_release(json, "0.2.0").unwrap().unwrap();
        assert!(r.assets.is_empty());
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.notes, "Screen sharing");
        assert_eq!(parse_release(json, "0.3.0").unwrap(), None);
        let pre = json.replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert_eq!(parse_release(&pre, "0.2.0").unwrap(), None);
    }

    #[test]
    fn old_program_names_map_to_new_ones() {
        let d = Path::new("dir");
        assert_eq!(
            new_name(&d.join("backroom.exe")),
            Some(d.join("TUFFcord.exe"))
        );
        assert_eq!(
            new_name(&d.join("Backroom.EXE")),
            Some(d.join("TUFFcord.EXE"))
        );
        assert_eq!(
            new_name(&d.join("backroom-server.exe")),
            Some(d.join("TUFFcord-server.exe"))
        );
        assert_eq!(
            new_name(&d.join("backroom-server")),
            Some(d.join("TUFFcord-server"))
        );
        assert_eq!(new_name(&d.join("TUFFcord.exe")), None);
        assert_eq!(new_name(&d.join("backroom-bot")), None);
        assert_eq!(new_name(&d.join("my-backroom.exe")), None);
    }
}
