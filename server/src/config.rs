//! Server settings: read from `TUFFcord-server.toml` (created on first run;
//! `backroom-server.toml` before the rename, which is renamed on first start),
//! with environment variables overriding the file.

use crate::log::Level;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub password: String,
    pub app_name: String,
    pub text_channels: Vec<String>,
    pub voice_channels: Vec<String>,
    pub max_per_voice_channel: usize,
    pub history_limit: usize,
    pub data_dir: PathBuf,
    pub log_level: Level,
    pub log_file: Option<PathBuf>,
    /// Look for newer versions on GitHub and mention them in the log.
    pub check_updates: bool,
    /// Install new versions by itself and restart when nobody's in voice.
    pub auto_update: bool,
    /// New accounts can be created (with the group password).
    pub allow_signup: bool,
    /// Largest file someone can send.
    pub max_attachment_bytes: u64,
    /// Most space all sent files may use together.
    pub max_storage_bytes: u64,
    /// Most one person can send in a day (0 = no limit).
    pub max_upload_bytes_per_day: u64,
    pub config_path: PathBuf,
    /// Notes to log once logging is running (first-run messages, bad values).
    pub notes: Vec<(Level, String)>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FileConfig {
    port: Option<u16>,
    host: Option<String>,
    password: Option<String>,
    app_name: Option<String>,
    text_channels: Option<Vec<String>>,
    voice_channels: Option<Vec<String>>,
    max_per_voice_channel: Option<usize>,
    history_limit: Option<usize>,
    data_dir: Option<String>,
    log_level: Option<String>,
    log_file: Option<String>,
    check_updates: Option<bool>,
    auto_update: Option<bool>,
    allow_signup: Option<bool>,
    max_attachment_mb: Option<u64>,
    max_storage_mb: Option<u64>,
    max_upload_mb_per_day: Option<u64>,
}

fn base_dir() -> PathBuf {
    proto::update::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn random_password() -> String {
    // Easy to read aloud: no 0/O or 1/l.
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("system random source unavailable");
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

fn default_file(password: &str) -> String {
    format!(
        r#"# TUFFcord server settings. Restart the server after changing anything here.

# Group password: people need it once, to create their account. After that they
# sign in with their own name and password. Leave it empty ("") to let anyone with
# the address create an account.
password = "{password}"

# Set to false to stop new accounts being created (once everyone has one).
allow_signup = true

port = 3000
app_name = "TUFFcord"
text_channels = ["general", "links", "memes"]
voice_channels = ["Lounge", "Gaming", "Quiet room"]
max_per_voice_channel = 8

# Recent chat messages per text channel, sent when someone signs in. Every
# older message is kept too (in data/history) and loads as people scroll up.
history_limit = 300

# Largest file people can send, in megabytes. Files are kept in data/attachments.
max_attachment_mb = 100

# Most space all sent files may use together, in megabytes. When it's full, the
# oldest files are deleted to make room (their messages stay, marked "expired").
# Admins are warned in the app from 80% (and when the disk is 80% full).
max_storage_mb = 5120

# Most one person can send in a day, in megabytes (0 = no limit). Stops one
# account from pushing everyone's older files out.
max_upload_mb_per_day = 3072

# How much to log: trace, debug, info, warn, error, critical.
# You can also change it while the server runs by typing: level debug
log_level = "info"

# Set to "" to log only to this window.
log_file = "data/TUFFcord.log"

# Check GitHub for a newer TUFFcord.
check_updates = true

# Install new versions by itself, then restart once nobody is in voice (people
# reconnect on their own). Set to false to just be told, then type "update" to install.
auto_update = true
"#
    )
}

/// `TUFFcord-server.toml` next to the server. Backroom (before 0.8) called it
/// `backroom-server.toml`; that one is renamed the first time.
fn settings_file(base: &Path, notes: &mut Vec<(Level, String)>) -> PathBuf {
    let new = base.join("TUFFcord-server.toml");
    let old = base.join("backroom-server.toml");
    if new.exists() || !old.exists() {
        return new;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => {
            notes.push((
                Level::Info,
                format!(
                    "Renamed backroom-server.toml to {} (Backroom is now TUFFcord).",
                    new.display()
                ),
            ));
            new
        }
        Err(e) => {
            notes.push((
                Level::Warn,
                format!("Couldn't rename backroom-server.toml to TUFFcord-server.toml ({e}); still using the old name."),
            ));
            old
        }
    }
}

/// Settings files from before the rename call the app Backroom. Lines still at
/// their old default switch to TUFFcord (the log file too; [`move_old_log`]
/// renames the file itself); a name someone chose is left alone.
fn rename_in_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut new = text.clone();
    for (old_line, new_line) in [
        ("app_name = \"Backroom\"", "app_name = \"TUFFcord\""),
        ("# Backroom server settings.", "# TUFFcord server settings."),
        (
            "# Check GitHub for a newer Backroom.",
            "# Check GitHub for a newer TUFFcord.",
        ),
        (
            "log_file = \"data/backroom.log\"",
            "log_file = \"data/TUFFcord.log\"",
        ),
    ] {
        new = new
            .split_inclusive('\n')
            .map(|line| {
                if line.trim_end_matches(['\r', '\n']).starts_with(old_line) {
                    line.replacen(old_line, new_line, 1)
                } else {
                    line.to_string()
                }
            })
            .collect();
    }
    if new == text {
        return None;
    }
    let renamed_app =
        !text.contains("app_name = \"TUFFcord\"") && new.contains("app_name = \"TUFFcord\"");
    std::fs::write(path, new).ok()?;
    Some(if renamed_app {
        format!(
            "Updated {}: app_name is now \"TUFFcord\" (it was the old default, \"Backroom\").",
            path.display()
        )
    } else {
        format!("Updated {} for the new name.", path.display())
    })
}

/// Servers set up before the rename logged to `backroom.log`. When the log is
/// now `TUFFcord.log` in the same folder and doesn't exist yet, the old log (and
/// its `.1` copy) is renamed so it carries on in one file.
fn move_old_log(log: &Path) -> Option<String> {
    if !log.file_name()?.eq_ignore_ascii_case("TUFFcord.log") || log.exists() {
        return None;
    }
    let old = log.with_file_name("backroom.log");
    std::fs::rename(&old, log).ok()?;
    let _ = std::fs::rename(
        old.with_file_name("backroom.log.1"),
        log.with_file_name("TUFFcord.log.1"),
    );
    Some(format!(
        "Renamed the log {} to {}.",
        old.display(),
        log.display()
    ))
}

/// Settings files written by 0.6.0 and earlier say images are limited to 8 MB.
/// If that part is untouched, switch it to the new defaults (100 MB per file, any
/// file type, 5 GB in all). A limit someone chose themselves is left alone.
fn upgrade_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let crlf = text.contains("\r\n");
    let unix = text.replace("\r\n", "\n");
    if !unix.contains(OLD_ATTACHMENT_BLOCK) || unix.contains("max_storage_mb") {
        return None;
    }
    let mut new = unix.replace(OLD_ATTACHMENT_BLOCK, NEW_ATTACHMENT_BLOCK);
    if crlf {
        new = new.replace('\n', "\r\n");
    }
    std::fs::write(path, new).ok()?;
    Some(format!(
        "Updated {}: people can now send any file up to 100 MB (it was images up to 8 MB), with 5 GB for files in all. Change max_attachment_mb and max_storage_mb there if you like.",
        path.display()
    ))
}

const OLD_ATTACHMENT_BLOCK: &str = r#"# Largest image people can send, in megabytes. Images are kept in data/attachments
# and deleted when their message ages out of history.
max_attachment_mb = 8
"#;
const NEW_ATTACHMENT_BLOCK: &str = r#"# Largest file people can send, in megabytes. Files are kept in data/attachments.
max_attachment_mb = 100

# Most space all sent files may use together, in megabytes. When it's full, the
# oldest files are deleted to make room (their messages stay, marked "expired").
# Admins are warned in the app from 80% (and when the disk is 80% full).
max_storage_mb = 5120
"#;

pub(crate) fn clean_channel(c: &str) -> String {
    c.trim()
        .to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// Write the channel lists into the settings file (channels added from the
/// app), replacing the `text_channels` and `voice_channels` settings or adding
/// them at the end. The rest of the file, comments included, stays as it is.
/// Environment variables still win over the file.
pub fn save_channels(path: &Path, text: &[String], voice: &[String]) -> std::io::Result<()> {
    // No file yet (settings from environment variables): start from the
    // default one, as a first run would have.
    let old = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => default_file(&random_password()),
        r => r?,
    };
    let nl = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let line = |key: &str, list: &[String]| {
        let items: Vec<String> = list
            .iter()
            .map(|c| serde_json::to_string(c).expect("serialize"))
            .collect();
        // JSON string escapes are valid TOML basic strings.
        format!("{key} = [{}]", items.join(", "))
    };
    let lines = [line("text_channels", text), line("voice_channels", voice)];
    let mut done = [false; 2];
    let mut in_list = false; // inside a list split over several lines
    let mut out = String::new();
    for l in old.lines() {
        if in_list {
            in_list = !l.contains(']');
            continue;
        }
        let key = l.split('=').next().unwrap_or("").trim();
        if let Some(i) = ["text_channels", "voice_channels"]
            .iter()
            .position(|k| *k == key && l.contains('='))
        {
            if !done[i] {
                out += &lines[i];
                out += nl;
                done[i] = true;
            }
            in_list = l.contains('[') && !l.contains(']');
            continue;
        }
        out += l;
        out += nl;
    }
    for i in 0..2 {
        if !done[i] {
            out += &lines[i];
            out += nl;
        }
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)
}

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty() || key == "PASSWORD" || key == "LOG_FILE")
}
fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

pub fn load() -> Config {
    let mut notes = Vec::new();
    let base = base_dir();
    let config_path = match std::env::var_os("BACKROOM_CONFIG") {
        Some(p) => PathBuf::from(p),
        None => settings_file(&base, &mut notes),
    };

    let mut file = FileConfig::default();
    if let Some(note) = upgrade_file(&config_path) {
        notes.push((Level::Info, note));
    }
    if let Some(note) = rename_in_file(&config_path) {
        notes.push((Level::Info, note));
    }
    if config_path.exists() {
        match std::fs::read_to_string(&config_path)
            .map_err(|e| e.to_string())
            .and_then(|s| toml::from_str::<FileConfig>(&s).map_err(|e| e.to_string()))
        {
            Ok(f) => file = f,
            Err(e) => notes.push((
                Level::Error,
                format!(
                    "Could not read {} ({e}). Using default settings.",
                    config_path.display()
                ),
            )),
        }
    } else if std::env::var_os("PASSWORD").is_none() {
        let pw = random_password();
        match std::fs::write(&config_path, default_file(&pw)) {
            Ok(()) => {
                notes.push((Level::Warn, format!("First run: created {} with the group password \"{pw}\". Change it there if you like.", config_path.display())));
                file.password = Some(pw);
            }
            Err(e) => notes.push((
                Level::Warn,
                format!(
                    "Could not create {} ({e}). Using default settings.",
                    config_path.display()
                ),
            )),
        }
    }

    let parse_num =
        |key: &str, v: Option<String>, notes: &mut Vec<(Level, String)>| -> Option<usize> {
            v.and_then(|s| match s.trim().parse() {
                Ok(n) => Some(n),
                Err(_) => {
                    notes.push((
                        Level::Warn,
                        format!("{key}=\"{s}\" isn't a number; ignoring it."),
                    ));
                    None
                }
            })
        };

    let port = parse_num("PORT", env("PORT"), &mut notes)
        .map(|n| n as u16)
        .or(file.port)
        .unwrap_or(3000);
    let host = env("HOST")
        .or(file.host)
        .unwrap_or_else(|| "0.0.0.0".into());
    let password = env("PASSWORD").or(file.password).unwrap_or_default();
    let app_name = env("APP_NAME")
        .or(file.app_name)
        .unwrap_or_else(|| "TUFFcord".into());
    let mut text_channels: Vec<String> = env("TEXT_CHANNELS")
        .map(|s| list(&s))
        .or(file.text_channels)
        .unwrap_or_else(|| vec!["general".into(), "links".into(), "memes".into()]);
    text_channels = text_channels
        .iter()
        .map(|c| clean_channel(c))
        .filter(|c| !c.is_empty())
        .collect();
    text_channels.dedup();
    if text_channels.is_empty() {
        text_channels.push("general".into());
    }
    let mut voice_channels: Vec<String> = env("VOICE_CHANNELS")
        .map(|s| list(&s))
        .or(file.voice_channels)
        .unwrap_or_else(|| vec!["Lounge".into(), "Gaming".into(), "Quiet room".into()]);
    voice_channels = voice_channels
        .into_iter()
        .map(|c| c.trim().chars().take(40).collect::<String>())
        .filter(|c| !c.is_empty())
        .collect();
    voice_channels.dedup();
    if voice_channels.is_empty() {
        voice_channels.push("Lounge".into());
    }
    let max_per_voice_channel = parse_num("MAX_PER_VOICE", env("MAX_PER_VOICE"), &mut notes)
        .or(file.max_per_voice_channel)
        .unwrap_or(8)
        .clamp(2, 32);
    let history_limit = parse_num("HISTORY_LIMIT", env("HISTORY_LIMIT"), &mut notes)
        .or(file.history_limit)
        .unwrap_or(300)
        .clamp(10, 10_000);

    let resolve = |p: String| -> PathBuf {
        let p = PathBuf::from(p);
        if p.is_absolute() {
            p
        } else {
            base.join(p)
        }
    };
    let data_dir = resolve(
        env("DATA_DIR")
            .or(file.data_dir)
            .unwrap_or_else(|| "data".into()),
    );

    let level_str = env("LOG_LEVEL")
        .or(file.log_level)
        .unwrap_or_else(|| "info".into());
    let log_level = Level::parse(&level_str).unwrap_or_else(|| {
        notes.push((Level::Warn, format!("Unknown log level \"{level_str}\", using info. Options: trace, debug, info, warn, error, critical")));
        Level::Info
    });
    let log_file = match env("LOG_FILE").or(file.log_file) {
        Some(s)
            if s.is_empty()
                || ["off", "none", "false", "0"].contains(&s.to_lowercase().as_str()) =>
        {
            None
        }
        Some(s) => Some(resolve(s)),
        None => Some(data_dir.join("TUFFcord.log")),
    };
    if let Some(note) = log_file.as_deref().and_then(move_old_log) {
        notes.push((Level::Info, note));
    }

    let check_updates = match env("CHECK_UPDATES") {
        Some(v) => !["0", "false", "no", "off"].contains(&v.to_lowercase().as_str()),
        None => file.check_updates.unwrap_or(true),
    };

    let auto_update = match env("AUTO_UPDATE") {
        Some(v) => !["0", "false", "no", "off"].contains(&v.to_lowercase().as_str()),
        None => file.auto_update.unwrap_or(true),
    };

    let allow_signup = match env("ALLOW_SIGNUP") {
        Some(v) => !["0", "false", "no", "off"].contains(&v.to_lowercase().as_str()),
        None => file.allow_signup.unwrap_or(true),
    };

    let max_attachment_mb = parse_num("MAX_ATTACHMENT_MB", env("MAX_ATTACHMENT_MB"), &mut notes)
        .map(|n| n as u64)
        .or(file.max_attachment_mb)
        .unwrap_or(100)
        .clamp(1, 2048);
    let max_storage_mb = parse_num("MAX_STORAGE_MB", env("MAX_STORAGE_MB"), &mut notes)
        .map(|n| n as u64)
        .or(file.max_storage_mb)
        .unwrap_or(5120)
        .max(max_attachment_mb);
    if password.is_empty() && allow_signup {
        notes.push((Level::Warn, "The group password is empty, so anyone who knows this server's address can create an account. Set password in the settings file, or allow_signup = false.".into()));
    }
    let max_upload_mb_per_day = parse_num(
        "MAX_UPLOAD_MB_PER_DAY",
        env("MAX_UPLOAD_MB_PER_DAY"),
        &mut notes,
    )
    .map(|n| n as u64)
    .or(file.max_upload_mb_per_day)
    .unwrap_or(3072);

    Config {
        max_upload_bytes_per_day: max_upload_mb_per_day * 1024 * 1024,
        max_attachment_bytes: max_attachment_mb * 1024 * 1024,
        max_storage_bytes: max_storage_mb * 1024 * 1024,
        check_updates,
        auto_update,
        allow_signup,
        port,
        host,
        password,
        app_name,
        text_channels,
        voice_channels,
        max_per_voice_channel,
        history_limit,
        data_dir,
        log_level,
        log_file,
        config_path,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_channel_lists_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("tc-chan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.toml");
        std::fs::write(
            &path,
            "# top\r\nport = 3000\r\ntext_channels = [\r\n  \"general\",\r\n]\r\n# end\r\n",
        )
        .unwrap();
        let voice = vec!["Lounge".to_string(), "Say \"hi\"".to_string()];
        save_channels(&path, &["general".into(), "clips".into()], &voice).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "# top\r\nport = 3000\r\ntext_channels = [\"general\", \"clips\"]\r\n# end\r\nvoice_channels = [\"Lounge\", \"Say \\\"hi\\\"\"]\r\n"
        );
        let parsed: FileConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed.voice_channels, Some(voice));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upgrades_untouched_old_settings_only() {
        let dir = std::env::temp_dir().join(format!("br-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("TUFFcord-server.toml");
        let new = default_file("pw");
        let old = new.replace(NEW_ATTACHMENT_BLOCK, OLD_ATTACHMENT_BLOCK);
        assert_ne!(old, new);

        // A file written by 0.6 (Windows line endings too) gets the new defaults.
        for text in [old.clone(), old.replace('\n', "\r\n")] {
            std::fs::write(&path, &text).unwrap();
            assert!(upgrade_file(&path).is_some());
            let got = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            assert_eq!(got, new);
            let parsed: FileConfig = toml::from_str(&got).unwrap();
            assert_eq!(
                (parsed.max_attachment_mb, parsed.max_storage_mb),
                (Some(100), Some(5120))
            );
            // Only once.
            assert!(upgrade_file(&path).is_none());
        }

        // A limit someone chose is left alone.
        let custom = old.replace("max_attachment_mb = 8", "max_attachment_mb = 20");
        std::fs::write(&path, &custom).unwrap();
        assert!(upgrade_file(&path).is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_settings_file_is_renamed_and_its_default_name_updated() {
        let dir = std::env::temp_dir().join(format!("tuff-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old_text = default_file("secret")
            .replace("data/TUFFcord.log", "data/backroom.log")
            .replace("TUFFcord", "Backroom")
            .replace('\n', "\r\n");
        std::fs::write(dir.join("backroom-server.toml"), &old_text).unwrap();
        let mut notes = Vec::new();
        let path = settings_file(&dir, &mut notes);
        assert_eq!(path, dir.join("TUFFcord-server.toml"));
        assert!(!dir.join("backroom-server.toml").exists());
        assert!(rename_in_file(&path).unwrap().contains("app_name"));
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(got.contains("app_name = \"TUFFcord\"\r\n"));
        assert!(got.starts_with("# TUFFcord server settings."));
        assert!(got.contains("# Check GitHub for a newer TUFFcord.\r\n"));
        assert!(got.contains("password = \"secret\""));
        assert!(
            got.contains("log_file = \"data/TUFFcord.log\""),
            "log is renamed too"
        );
        assert_eq!(got.matches("Backroom").count(), 0);
        let parsed: FileConfig = toml::from_str(&got).unwrap();
        assert_eq!(parsed.app_name.as_deref(), Some("TUFFcord"));
        assert!(rename_in_file(&path).is_none(), "only once");

        // A name someone chose stays.
        std::fs::write(&path, "app_name = \"Backroom Crew\"\npassword = \"x\"\n").unwrap();
        assert!(rename_in_file(&path).is_none());
        // Both files there: the new one wins, the old one is left alone.
        std::fs::write(dir.join("backroom-server.toml"), "x").unwrap();
        assert_eq!(settings_file(&dir, &mut notes), path);
        assert!(dir.join("backroom-server.toml").exists());

        // The old log carries on under the new name, once.
        std::fs::write(dir.join("backroom.log"), "old").unwrap();
        std::fs::write(dir.join("backroom.log.1"), "older").unwrap();
        let log = dir.join("TUFFcord.log");
        assert!(move_old_log(&log).is_some());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "old");
        assert_eq!(
            std::fs::read_to_string(dir.join("TUFFcord.log.1")).unwrap(),
            "older"
        );
        std::fs::write(dir.join("backroom.log"), "again").unwrap();
        assert!(move_old_log(&log).is_none());
        assert!(move_old_log(&dir.join("custom.log")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
