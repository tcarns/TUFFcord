//! Network thread: one WebSocket to the server, reconnecting automatically.
//! Incoming voice is decoded here and handed straight to the mixer; everything
//! else becomes a `NetEvent` for the UI.

use crate::voice::{Mixer, Receiver};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use proto::{Auth, ClientMsg, ServerMsg, FATAL_ERRORS, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Bytes, Message};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How to sign in.
#[derive(Clone, Debug, PartialEq)]
pub enum SignIn {
    /// The old way: a name and the group password. Servers from before accounts
    /// accept it; newer ones answer "account_required".
    Group { password: String },
    /// Your account's name (the `name`) and password.
    Login { password: String },
    /// Create an account. On a server from before accounts this signs in the old
    /// way with the group password instead.
    Register {
        group_password: String,
        new_password: String,
    },
    /// A sign-in the server handed out earlier.
    Token { token: String },
}

pub enum NetCmd {
    Connect {
        url: String,
        name: String,
        sign_in: SignIn,
    },
    Disconnect,
    Send(ClientMsg),
    Voice(Vec<u8>),
    /// An image upload frame (proto::encode_upload).
    Upload(Vec<u8>),
}

#[derive(Debug)]
pub enum NetEvent {
    Connecting,
    Server(ServerMsg),
    /// Lost the connection; trying again shortly.
    Reconnecting {
        reason: String,
    },
    /// Stopped trying (wrong password, unreachable on first try, etc.).
    /// `code` is the server's reason, when it gave one (e.g. "session_expired").
    Failed {
        message: String,
        code: Option<String>,
    },
    Ping(u32),
    /// Bytes of an attachment we asked for.
    Attachment {
        id: String,
        bytes: Vec<u8>,
    },
}

#[derive(Clone)]
pub struct VoiceSender(mpsc::UnboundedSender<NetCmd>);
impl VoiceSender {
    pub fn send(&self, frame: Vec<u8>) {
        let _ = self.0.send(NetCmd::Voice(frame));
    }
}

pub struct Net {
    cmd: mpsc::UnboundedSender<NetCmd>,
    events: std_mpsc::Receiver<NetEvent>,
}

impl Net {
    pub fn spawn(mixer: Arc<Mutex<Mixer>>, wake: Wake) -> Net {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = std_mpsc::channel();
        std::thread::Builder::new()
            .name("net".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(run(cmd_rx, ev_tx, mixer, wake));
            })
            .expect("spawn net thread");
        Net {
            cmd: cmd_tx,
            events: ev_rx,
        }
    }
    pub fn connect(&self, url: String, name: String, sign_in: SignIn) {
        let _ = self.cmd.send(NetCmd::Connect { url, name, sign_in });
    }
    pub fn disconnect(&self) {
        let _ = self.cmd.send(NetCmd::Disconnect);
    }
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.cmd.send(NetCmd::Send(msg));
    }
    pub fn upload(&self, frame: Vec<u8>) {
        let _ = self.cmd.send(NetCmd::Upload(frame));
    }
    pub fn voice_sender(&self) -> VoiceSender {
        VoiceSender(self.cmd.clone())
    }
    pub fn try_recv(&self) -> Option<NetEvent> {
        self.events.try_recv().ok()
    }
}

/// Turns what people type ("abc.trycloudflare.com", "192.168.1.5:3000", a full URL)
/// into a WebSocket URL.
pub fn normalize_server(input: &str) -> Result<String, String> {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        return Err("Enter the server address.".into());
    }
    let (scheme, rest) = if let Some(r) = s.strip_prefix("wss://") {
        ("wss", r)
    } else if let Some(r) = s.strip_prefix("ws://") {
        ("ws", r)
    } else if let Some(r) = s.strip_prefix("https://") {
        ("wss", r)
    } else if let Some(r) = s.strip_prefix("http://") {
        ("ws", r)
    } else {
        let host = s.split(['/', ':']).next().unwrap_or("");
        let local = host == "localhost"
            || host.parse::<std::net::IpAddr>().is_ok()
            || host.ends_with(".local")
            || !host.contains('.');
        (if local { "ws" } else { "wss" }, s)
    };
    if rest.is_empty() || rest.contains(' ') {
        return Err("That doesn't look like a server address.".into());
    }
    let with_path = if rest.contains('/') {
        rest.to_string()
    } else {
        format!("{rest}/ws")
    };
    let url = format!("{scheme}://{with_path}");
    // A bare local name like "myserver" also needs a port to be useful; leave that to the user.
    Ok(url)
}

/// True if signing in to `input` would send the password unencrypted over the
/// internet: a `ws://` address that isn't this PC or the home network.
pub fn unencrypted_public(input: &str) -> bool {
    let Ok(url) = normalize_server(input) else {
        return false;
    };
    let Some(rest) = url.strip_prefix("ws://") else {
        return false;
    };
    let hostport = rest.split('/').next().unwrap_or("");
    let host = if let Some(h) = hostport.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.octets()[0] == 100 && ip.octets()[1] & 0xC0 == 64)
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            !(ip.is_loopback()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80)
        }
        Err(_) => host != "localhost" && !host.ends_with(".local") && host.contains('.'),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

enum SessionEnd {
    Lost(String),
    UserDisconnect,
    NewTarget(String, String, SignIn),
    Fatal(String, Option<String>),
}

struct Target {
    url: String,
    name: String,
    sign_in: SignIn,
    welcomed: bool,
}

async fn run(
    mut cmd_rx: mpsc::UnboundedReceiver<NetCmd>,
    ev: std_mpsc::Sender<NetEvent>,
    mixer: Arc<Mutex<Mixer>>,
    wake: Wake,
) {
    let emit = |e: NetEvent| {
        let _ = ev.send(e);
        wake();
    };
    let mut target: Option<Target> = None;
    let mut delay = Duration::from_secs(1);
    loop {
        // Idle until asked to connect.
        let Some(t) = target.as_mut() else {
            match cmd_rx.recv().await {
                Some(NetCmd::Connect { url, name, sign_in }) => {
                    target = Some(Target {
                        url,
                        name,
                        sign_in,
                        welcomed: false,
                    });
                    delay = Duration::from_secs(1);
                }
                Some(_) => {}
                None => return,
            }
            continue;
        };

        emit(NetEvent::Connecting);
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async_with_config(t.url.as_str(), None, true),
        )
        .await;
        let end = match attempt {
            Ok(Ok((ws, _))) => session(ws, t, &mut cmd_rx, &emit, &mixer, &wake).await,
            Ok(Err(e)) => SessionEnd::Lost(describe_connect_error(&e.to_string())),
            Err(_) => SessionEnd::Lost("the server didn't answer within 10 seconds".into()),
        };
        mixer.lock().clear();
        match end {
            SessionEnd::UserDisconnect => target = None,
            SessionEnd::NewTarget(url, name, sign_in) => {
                target = Some(Target {
                    url,
                    name,
                    sign_in,
                    welcomed: false,
                });
                delay = Duration::from_secs(1);
            }
            SessionEnd::Fatal(message, code) => {
                emit(NetEvent::Failed { message, code });
                target = None;
            }
            SessionEnd::Lost(reason) => {
                if !t.welcomed {
                    emit(NetEvent::Failed {
                        message: format!("Couldn't connect: {reason}."),
                        code: None,
                    });
                    target = None;
                    continue;
                }
                emit(NetEvent::Reconnecting { reason });
                // Wait before retrying, but stay responsive to commands.
                let deadline = tokio::time::Instant::now() + delay;
                delay = (delay.mul_f32(1.7)).min(Duration::from_secs(15));
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        cmd = cmd_rx.recv() => match cmd {
                            Some(NetCmd::Disconnect) => { target = None; break; }
                            Some(NetCmd::Connect { url, name, sign_in }) => {
                                target = Some(Target { url, name, sign_in, welcomed: false });
                                delay = Duration::from_secs(1);
                                break;
                            }
                            Some(_) => {} // drop chat/voice while offline
                            None => return,
                        }
                    }
                }
            }
        }
    }
}

fn describe_connect_error(e: &str) -> String {
    let lower = e.to_lowercase();
    if lower.contains("refused") {
        "nothing is running at that address (is the server started?)".into()
    } else if lower.contains("dns")
        || lower.contains("resolve")
        || lower.contains("no such host")
        || lower.contains("failed to lookup")
    {
        "that address couldn't be found (check for typos)".into()
    } else if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
        format!("a secure connection couldn't be set up ({e})")
    } else if lower.contains("404") || lower.contains("http error") {
        "something answered, but it isn't a TUFFcord server".into()
    } else {
        e.to_string()
    }
}

async fn session<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    target: &mut Target,
    cmd_rx: &mut mpsc::UnboundedReceiver<NetCmd>,
    emit: &impl Fn(NetEvent),
    mixer: &Arc<Mutex<Mixer>>,
    wake: &Wake,
) -> SessionEnd
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    let (password, auth) = match &target.sign_in {
        SignIn::Group { password } => (password.clone(), None),
        SignIn::Login { password } => (password.clone(), Some(Auth::Login)),
        SignIn::Register {
            group_password,
            new_password,
        } => (
            group_password.clone(),
            Some(Auth::Register {
                new_password: new_password.clone(),
            }),
        ),
        SignIn::Token { token } => (
            String::new(),
            Some(Auth::Token {
                token: token.clone(),
            }),
        ),
    };
    let hello = ClientMsg::Hello {
        name: target.name.clone(),
        password,
        version: PROTOCOL_VERSION,
        accounts: true,
        auth,
    };
    if sink
        .send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .is_err()
    {
        return SessionEnd::Lost("the connection closed right away".into());
    }

    let mut receiver = Receiver::default();
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut ping = tokio::time::interval(Duration::from_secs(5));
    let mut last_heard = Instant::now();
    let mut quality_check = tokio::time::interval(Duration::from_secs(60));
    quality_check.tick().await;

    loop {
        tokio::select! {
            msg = stream.next() => {
                last_heard = Instant::now();
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if b.first() == Some(&proto::ATTACH_DATA) {
                            if let Some((id, bytes)) = proto::parse_attachment_data(&b) {
                                emit(NetEvent::Attachment { id: id.to_string(), bytes: bytes.to_vec() });
                            }
                        } else if let Some((kind, sender, seq, payload)) = proto::parse_server_voice(&b) {
                            if receiver.handle(kind, sender, seq, payload, mixer) {
                                wake();
                            }
                        }
                    }
                    Some(Ok(Message::Text(t))) => {
                        let Ok(m) = serde_json::from_str::<ServerMsg>(t.as_str()) else { continue };
                        match &m {
                            ServerMsg::Error { code, message } if FATAL_ERRORS.contains(&code.as_str()) => {
                                // Let the UI see the message too, then stop.
                                return SessionEnd::Fatal(message.clone(), Some(code.clone()));
                            }
                            ServerMsg::Welcome { voice_state, token, name, .. } => {
                                target.welcomed = true;
                                // Reconnect with the server's sign-in from now on, so a
                                // dropped connection never needs the password again.
                                if let Some(t) = token {
                                    target.sign_in = SignIn::Token { token: t.clone() };
                                    target.name = name.clone();
                                }
                                update_names(&mut names, voice_state);
                            }
                            ServerMsg::AccountUpdated { account, token } => {
                                target.name = account.name.clone();
                                if let Some(t) = token {
                                    target.sign_in = SignIn::Token { token: t.clone() };
                                }
                            }
                            ServerMsg::VoiceState { state } => {
                                update_names(&mut names, state);
                                receiver.retain(|id| names.contains_key(&id));
                                mixer.lock().retain(|id| names.contains_key(&id));
                            }
                            ServerMsg::Pong { t } => {
                                emit(NetEvent::Ping(now_ms().saturating_sub(*t) as u32));
                                continue;
                            }
                            _ => {}
                        }
                        emit(NetEvent::Server(m));
                    }
                    Some(Ok(Message::Close(_))) | None => return SessionEnd::Lost("the server closed the connection".into()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return SessionEnd::Lost(e.to_string()),
                }
            }
            cmd = cmd_rx.recv() => {
                let out = match cmd {
                    Some(NetCmd::Voice(frame)) => {
                        if !target.welcomed { continue; }
                        Message::Binary(Bytes::from(frame))
                    }
                    Some(NetCmd::Upload(frame)) => {
                        if !target.welcomed { continue; }
                        Message::Binary(Bytes::from(frame))
                    }
                    Some(NetCmd::Send(m)) => Message::Text(serde_json::to_string(&m).unwrap().into()),
                    Some(NetCmd::Disconnect) => {
                        let _ = sink.close().await;
                        return SessionEnd::UserDisconnect;
                    }
                    Some(NetCmd::Connect { url, name, sign_in }) => {
                        let _ = sink.close().await;
                        return SessionEnd::NewTarget(url, name, sign_in);
                    }
                    None => return SessionEnd::UserDisconnect,
                };
                if let Err(e) = sink.send(out).await {
                    return SessionEnd::Lost(e.to_string());
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > Duration::from_secs(30) {
                    return SessionEnd::Lost("the server stopped responding".into());
                }
                if target.welcomed {
                    let p = ClientMsg::Ping { t: now_ms() };
                    if sink.send(Message::Text(serde_json::to_string(&p).unwrap().into())).await.is_err() {
                        return SessionEnd::Lost("the connection dropped".into());
                    }
                }
            }
            _ = quality_check.tick() => {
                // Tell the server log if someone's audio kept breaking up for us.
                let underruns = mixer.lock().take_underruns();
                for (id, count) in underruns {
                    if count >= 5 {
                        let who = names.get(&id).cloned().unwrap_or_else(|| "someone".into());
                        let report = ClientMsg::Report {
                            kind: "audioQuality".into(),
                            message: format!("audio from {who} broke up {count} times in the last minute (network delay)"),
                        };
                        let _ = sink.send(Message::Text(serde_json::to_string(&report).unwrap().into())).await;
                    }
                }
            }
        }
    }
}

fn update_names(names: &mut HashMap<u32, String>, state: &[proto::VoiceChannelState]) {
    names.clear();
    for ch in state {
        for m in &ch.members {
            names.insert(m.id, m.name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_server, unencrypted_public};
    #[test]
    fn addresses() {
        assert_eq!(
            normalize_server("abc-def.trycloudflare.com").unwrap(),
            "wss://abc-def.trycloudflare.com/ws"
        );
        assert_eq!(
            normalize_server("https://abc.trycloudflare.com/").unwrap(),
            "wss://abc.trycloudflare.com/ws"
        );
        assert_eq!(
            normalize_server("localhost:3000").unwrap(),
            "ws://localhost:3000/ws"
        );
        assert_eq!(
            normalize_server("192.168.1.20:3000").unwrap(),
            "ws://192.168.1.20:3000/ws"
        );
        assert_eq!(
            normalize_server("ws://host:1/custom").unwrap(),
            "ws://host:1/custom"
        );
        assert!(unencrypted_public("5.161.12.34:3000"));
        assert!(unencrypted_public("http://chat.example.com"));
        assert!(!unencrypted_public("chat.example.com"), "wss");
        assert!(!unencrypted_public("localhost:3000"));
        assert!(!unencrypted_public("192.168.1.20:3000"));
        assert!(!unencrypted_public("myserver:3000"));
        assert!(normalize_server("  ").is_err());
    }
}
