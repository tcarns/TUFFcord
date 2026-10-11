//! Server console commands: typed in the server window, or (on Linux, where the
//! server runs as a service with no window) sent to the running server with
//! `TUFFcord-server <command>`, through a socket in the data folder that only
//! the server's own user can open.

use crate::log::{self, Level};
use crate::{admin, invites, selfupdate, trash, Shared};
use std::io::BufRead;
use std::sync::mpsc::Sender;

pub type Updates = Sender<selfupdate::Request>;

/// Run one command line. Returns what to show (None for an empty line).
pub fn run(shared: &Shared, updates: &Updates, line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let cmd = parts.next()?;
    let arg = parts.collect::<Vec<_>>().join(" ");
    Some(match cmd.to_lowercase().as_str() {
        "help" => format!(
            "Commands:\n  level              show the current log level\n  level <name>       change it now ({})\n  status             who is online and in voice, uptime, memory use\n  update             install the newest version now and restart\n{}\n{}\n{}\n  help               show this list",
            log::LEVEL_NAMES.join(", "),
            invites::CONSOLE_HELP,
            admin::CONSOLE_HELP,
            trash::CONSOLE_HELP
        ),
        "level" if arg.is_empty() => format!("Log level is \"{}\". Options, most to least detail: {}", log::level().name(), log::LEVEL_NAMES.join(", ")),
        "level" => match Level::parse(&arg) {
            Some(l) => {
                let before = log::level();
                log::set_level(l);
                let text = format!("Log level changed from {} to {}", before.name(), l.name());
                log::write(Level::Info, "server", &text, true);
                text
            }
            None => format!("\"{arg}\" isn't a log level. Options: {}", log::LEVEL_NAMES.join(", ")),
        },
        "status" => shared.lock().unwrap().status_text(),
        "update" => {
            let _ = updates.send(selfupdate::Request::Now);
            "Looking for a new version to install now.".into()
        }
        other => invites::console(shared, other)
            .or_else(|| admin::console(shared, other, &arg))
            .or_else(|| trash::console(shared, other, &arg))
            .unwrap_or_else(|| format!("Unknown command \"{cmd}\". Type \"help\" for the list.")),
    })
}

/// Read commands typed in the server window.
pub fn stdin(shared: Shared, updates: Updates) {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Some(out) = run(&shared, &updates, &line) {
            if !line.trim_start().to_lowercase().starts_with("level ") {
                println!("{out}"); // a level change is logged already
            }
        }
    }
}

#[cfg(unix)]
mod sock {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("console.sock")
    }

    /// Take commands from `TUFFcord-server <command>` while the server runs.
    pub fn listen(shared: Shared, updates: Updates, data_dir: &Path) {
        let p = path(data_dir);
        let _ = std::fs::remove_file(&p); // left over from the last run
        let listener = match UnixListener::bind(&p) {
            Ok(l) => l,
            Err(e) => {
                warn!(
                    "server",
                    "Commands from the command line won't work: couldn't open {} ({e})",
                    p.display()
                );
                return;
            }
        };
        // Only the server's own user (and root) may send commands.
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                let mut conn = conn;
                let mut line = String::new();
                if (&conn).take(4096).read_to_string(&mut line).is_err() {
                    continue;
                }
                let line = line.trim();
                info!(
                    "server",
                    "Command from the command line: {}",
                    crate::clean(line, 80)
                );
                let out = run(&shared, &updates, line).unwrap_or_default();
                let _ = conn.write_all(out.as_bytes());
            }
        });
    }

    /// `TUFFcord-server <command>`: hand it to the running server and print
    /// the answer. Exits.
    pub fn send(line: &str, data_dir: &Path) -> ! {
        let p = path(data_dir);
        let mut conn = match UnixStream::connect(&p) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Couldn't reach the running server through {} ({e}). Is it running, and are you its user (sudo -u tuffcord)?", p.display());
                std::process::exit(1);
            }
        };
        let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
        let mut out = String::new();
        let ok = conn.write_all(line.as_bytes()).is_ok()
            && conn.shutdown(std::net::Shutdown::Write).is_ok()
            && conn.read_to_string(&mut out).is_ok();
        if !ok {
            eprintln!("The server didn't answer.");
            std::process::exit(1);
        }
        println!("{out}");
        std::process::exit(0);
    }
}

/// Start taking commands: from the window, and on Linux from the command line.
pub fn start(shared: Shared, updates: Updates, data_dir: &std::path::Path) {
    #[cfg(unix)]
    sock::listen(shared.clone(), updates.clone(), data_dir);
    #[cfg(not(unix))]
    let _ = data_dir;
    std::thread::spawn(move || stdin(shared, updates));
}

/// Started with a command (`TUFFcord-server invite`): send it to the running
/// server and exit. Does nothing when started without one.
pub fn send_if_asked() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0].starts_with('-') {
        return;
    }
    #[cfg(unix)]
    sock::send(&args.join(" "), &crate::config::load().data_dir);
    #[cfg(not(unix))]
    {
        eprintln!("Type commands in the server window instead.");
        std::process::exit(1);
    }
}
