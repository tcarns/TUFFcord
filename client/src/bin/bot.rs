//! Headless test client. Uses the app's real network and voice code, but plays a
//! test tone instead of a microphone and measures what it hears.
//!
//! tuffcord-bot --server ws://127.0.0.1:3000/ws --name Bot1 --password pw --channel Lounge \
//!              --tone 440 --listen 660,880 --seconds 6 [--mute-after 3] [--deafen]

use parking_lot::Mutex;
use proto::{ClientMsg, ServerMsg, FRAME_SAMPLES, SAMPLE_RATE};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tuffcord::net::{Net, NetEvent, SignIn};
use tuffcord::voice::{Mixer, TxPipeline, VoiceControls};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Signal power at one frequency (Goertzel), in dB.
fn goertzel_db(samples: &[f32], freq: f32) -> f32 {
    let w = std::f32::consts::TAU * freq / SAMPLE_RATE as f32;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in samples {
        let s = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    let norm = power / (samples.len() as f32 * samples.len() as f32 / 4.0);
    10.0 * norm.max(1e-12).log10()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let server = arg(&args, "--server").unwrap_or_else(|| "ws://127.0.0.1:3000/ws".into());
    let name = arg(&args, "--name").unwrap_or_else(|| "Bot".into());
    // An invite (the group password on servers before 0.12). The bot creates an
    // account with it (or signs in, if the name is taken) with --account-password;
    // without one it just signs in. --legacy signs in the old way.
    let password = arg(&args, "--password").unwrap_or_default();
    let account_password =
        arg(&args, "--account-password").unwrap_or_else(|| format!("bot-{name}-password"));
    let legacy = args.iter().any(|a| a == "--legacy");
    let channel = arg(&args, "--channel").unwrap_or_else(|| "Lounge".into());
    let tone: f32 = arg(&args, "--tone")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let listen: Vec<f32> = arg(&args, "--listen")
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default();
    let seconds: f32 = arg(&args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5.0);
    let mute_after: Option<f32> = arg(&args, "--mute-after").and_then(|s| s.parse().ok());
    let deafen = args.iter().any(|a| a == "--deafen");
    let send_image = arg(&args, "--send-image");
    // Any file, over HTTP (servers from 0.7), with the app's own upload code.
    let send_file = arg(&args, "--send-file");
    let caption = arg(&args, "--caption").unwrap_or_default();
    let say: Vec<String> = arg(&args, "--say")
        .map(|s| s.split('|').map(str::to_string).collect())
        .unwrap_or_default();

    let mixer = Arc::new(Mutex::new(Mixer::default()));
    let net = Net::spawn(mixer.clone(), Arc::new(|| {}));
    let ctl = Arc::new(VoiceControls::default());
    ctl.noise_suppression.store(false, Ordering::Relaxed); // a pure tone isn't speech
    ctl.deafened.store(deafen, Ordering::Relaxed);
    mixer.lock().deafened = deafen;
    let voice = net.voice_sender();
    let mut tx = TxPipeline::new().expect("opus");

    let first = if legacy {
        SignIn::Group {
            password: password.clone(),
        }
    } else if password.is_empty() {
        SignIn::Login {
            password: account_password.clone(),
        }
    } else {
        SignIn::Register {
            group_password: password.clone(),
            new_password: account_password.clone(),
        }
    };
    net.connect(server.clone(), name.clone(), first);
    let mut tried_login = legacy || password.is_empty();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut my_id = 0;
    let mut endpoint: Option<tuffcord::files::Endpoint> = None;
    let mut joined = false;
    let mut errors: Vec<String> = Vec::new();
    while !joined && Instant::now() < deadline {
        while let Some(ev) = net.try_recv() {
            match ev {
                NetEvent::Server(ServerMsg::Welcome { id, file_key, .. }) => {
                    my_id = id;
                    endpoint = file_key.map(|key| tuffcord::files::Endpoint {
                        base: proto::files::http_base(&server),
                        key,
                    });
                    net.send(ClientMsg::JoinVoice {
                        channel: channel.clone(),
                        muted: false,
                        deafened: deafen,
                    });
                }
                NetEvent::Server(ServerMsg::VoiceJoined { .. }) => joined = true,
                NetEvent::Server(ServerMsg::Error { message, .. }) => errors.push(message),
                NetEvent::Failed { code, .. }
                    if code.as_deref() == Some("name_taken") && !tried_login =>
                {
                    tried_login = true;
                    net.connect(
                        server.clone(),
                        name.clone(),
                        SignIn::Login {
                            password: account_password.clone(),
                        },
                    );
                }
                NetEvent::Failed { message, code } => {
                    println!(
                        "{}",
                        serde_json::json!({ "name": name, "ok": false, "error": message, "code": code })
                    );
                    std::process::exit(2);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !joined {
        println!(
            "{}",
            serde_json::json!({ "name": name, "ok": false, "error": "never joined voice", "errors": errors })
        );
        std::process::exit(3);
    }
    ctl.in_voice.store(true, Ordering::Relaxed);
    for line in &say {
        net.send(ClientMsg::Chat {
            channel: "general".into(),
            text: line.clone(),
        });
        std::thread::sleep(Duration::from_millis(150));
    }
    let mut image_sent: Option<(String, usize)> = None;
    if let Some(path) = &send_image {
        match tuffcord::images::prepare_file(std::path::Path::new(path), 8 << 20) {
            Ok(p) => {
                let header = proto::UploadHeader {
                    channel: "general".into(),
                    text: caption.clone(),
                    name: p.name.clone(),
                    mime: p.mime.into(),
                    width: p.width,
                    height: p.height,
                };
                net.upload(proto::encode_upload(&header, &p.bytes));
                image_sent = Some((p.name, p.bytes.len()));
            }
            Err(e) => errors.push(e),
        }
    }
    let mut file_sent: Option<serde_json::Value> = None;
    if let Some(path) = &send_file {
        let path = std::path::PathBuf::from(path);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match &endpoint {
            None => errors.push("this server doesn't take files over HTTP".into()),
            Some(to) => {
                let started = Instant::now();
                let never = std::sync::atomic::AtomicBool::new(false);
                let updates = std::cell::Cell::new(0u32);
                let src = tuffcord::files::Source::Path(path.clone());
                match tuffcord::files::upload(
                    to,
                    &name,
                    &src,
                    &|_| updates.set(updates.get() + 1),
                    &never,
                ) {
                    Ok(upload) => {
                        net.send(ClientMsg::Post {
                            channel: "general".into(),
                            text: caption.clone(),
                            files: vec![proto::PostFile {
                                upload,
                                ..Default::default()
                            }],
                        });
                        file_sent = Some(serde_json::json!({
                            "name": name, "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                            "seconds": started.elapsed().as_secs_f32(), "progressUpdates": updates.get(),
                        }));
                    }
                    Err(e) => errors.push(e),
                }
            }
        }
    }
    let mut files_seen: Vec<serde_json::Value> = Vec::new();
    // Images others post while we're here: fetch each one and check the size matches.
    let mut images_seen: Vec<serde_json::Value> = Vec::new();
    let mut expected: std::collections::HashMap<String, (String, u64)> =
        std::collections::HashMap::new();

    let start = Instant::now();
    let mut next = start;
    let mut phase = 0usize;
    let mut heard: Vec<f32> = Vec::new();
    let mut sent = 0u32;
    let mut sent_after_mute = 0u32;
    let mut out = vec![0.0f32; FRAME_SAMPLES];
    let mut speaking_seen: BTreeMap<u32, u32> = BTreeMap::new();
    let mut pings = Vec::new();
    let mut muted = false;
    while start.elapsed() < Duration::from_secs_f32(seconds) {
        if let Some(t) = mute_after {
            if !muted && start.elapsed() > Duration::from_secs_f32(t) {
                muted = true;
                ctl.muted.store(true, Ordering::Relaxed);
                net.send(ClientMsg::Status {
                    muted: true,
                    deafened: deafen,
                });
            }
        }
        // Mic: 20 ms of tone (or silence).
        let frame: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| {
                if tone > 0.0 {
                    (std::f32::consts::TAU * tone * (phase + i) as f32 / SAMPLE_RATE as f32).sin()
                        * 0.3
                } else {
                    0.0
                }
            })
            .collect();
        phase += FRAME_SAMPLES;
        tx.push(&frame, &ctl, &mut |f| {
            if muted {
                sent_after_mute += 1;
            }
            sent += 1;
            voice.send(f);
        });
        // Speaker: pull 20 ms from the mixer.
        {
            let mut m = mixer.lock();
            m.mix(&mut out);
            for id in m.speaking() {
                *speaking_seen.entry(id).or_default() += 1;
            }
        }
        heard.extend_from_slice(&out);
        while let Some(ev) = net.try_recv() {
            match ev {
                NetEvent::Ping(ms) => pings.push(ms),
                NetEvent::Server(ServerMsg::Chat { message }) => {
                    for a in &message.attachments {
                        expected.insert(a.id.clone(), (a.name.clone(), a.size));
                        match &endpoint {
                            // Servers from 0.7: download over HTTP with the app's code.
                            Some(from) => {
                                let dest = std::env::temp_dir().join(format!(
                                    "bot-{}-{}",
                                    std::process::id(),
                                    a.id
                                ));
                                let never = std::sync::atomic::AtomicBool::new(false);
                                let r = tuffcord::files::download(
                                    from,
                                    &a.id,
                                    &dest,
                                    &|_, _| {},
                                    &never,
                                );
                                let bytes = std::fs::read(&dest).unwrap_or_default();
                                let _ = std::fs::remove_file(&dest);
                                files_seen.push(serde_json::json!({
                                    "name": a.name, "mime": a.mime, "bytes": bytes.len(),
                                    "sizeMatches": bytes.len() as u64 == a.size, "error": r.err(),
                                }));
                            }
                            None => net.send(ClientMsg::GetAttachment { id: a.id.clone() }),
                        }
                    }
                }
                NetEvent::Attachment { id, bytes } => {
                    let (name, size) = expected.get(&id).cloned().unwrap_or_default();
                    let decodes = tuffcord::images::decode_fit(&bytes, 64, 64).is_ok();
                    images_seen.push(serde_json::json!({ "name": name, "bytes": bytes.len(), "sizeMatches": bytes.len() as u64 == size, "decodes": decodes }));
                }
                NetEvent::Server(ServerMsg::Error { message, .. }) => errors.push(message),
                _ => {}
            }
        }
        next += Duration::from_millis(20);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }

    // Analyze the second half, when everyone is surely talking (or the first/second half around a mute).
    let half = heard.len() / 2;
    let levels: BTreeMap<String, f32> = listen
        .iter()
        .map(|f| {
            (
                format!("{f}"),
                (goertzel_db(&heard[half..], *f) * 10.0).round() / 10.0,
            )
        })
        .collect();
    let first_half: BTreeMap<String, f32> = listen
        .iter()
        .map(|f| {
            (
                format!("{f}"),
                (goertzel_db(&heard[FRAME_SAMPLES * 25..half], *f) * 10.0).round() / 10.0,
            )
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "name": name, "ok": true, "id": my_id, "framesSent": sent, "framesSentWhileMuted": sent_after_mute,
            "heardSecondHalfDb": levels, "heardFirstHalfDb": first_half,
            "speakingSeen": speaking_seen, "pingsMs": pings,
            "imageSent": image_sent.map(|(n, b)| serde_json::json!({ "name": n, "bytes": b })), "imagesReceived": images_seen, "errors": errors,
            "fileSent": file_sent, "filesReceived": files_seen,
        })
    );
    net.disconnect();
    std::thread::sleep(Duration::from_millis(200));
}
