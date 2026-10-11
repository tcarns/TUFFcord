//! TUFFcord desktop app: startup, the `App` state and the frame loop. Each area
//! of the UI has its own module with its own `impl App` block (see "Layout" and
//! "Where new code goes" in CLAUDE.md); put new features there, not here.

// No console window behind the app in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod accounts;
mod attach;
mod attach_ui;
mod autoscroll;
mod channels_ui;
mod chat;
mod chat_text;
mod dialogs;
mod downloads;
mod emoji_picker;
mod history_ui;
mod members;
mod posters;
mod security_ui;
mod server_msgs;
mod settings_window;
mod sidebar;
mod theme;
mod update_ui;
mod volume_ui;
mod widgets;

use attach::Attachments;
use eframe::egui::{self, Key};
use parking_lot::Mutex;
use proto::update::{self as updates, Release};
use proto::{Attachment, ChatMessage, ClientMsg, Member, User, VoiceChannelState};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::pal;
use tuffcord::audio::{self, DeviceList};
use tuffcord::emoji;
use tuffcord::keys::{self, GlobalKeys};
use tuffcord::net::{Net, Wake};
use tuffcord::settings::Settings;
use tuffcord::updater::{self, Phase, Updater};
use tuffcord::voice::{chime, Mixer, VoiceControls};

// ---------------------------------------------------------------- palette

/// `TUFFcord --check-media <file> <report.txt>`: read a video or audio file's
/// details and play it for a few seconds without a window, writing what
/// happened to the report. For checking playback problems on a PC.
fn check_media(file: &std::path::Path, report: &std::path::Path) {
    let mut out = String::new();
    match tuffcord::media::probe(file) {
        Ok(p) => out.push_str(&format!(
            "probe: {}x{}, {} ms, poster: {}\n",
            p.width,
            p.height,
            p.duration_ms,
            p.poster.as_ref().map_or("none".to_string(), |i| format!(
                "{}x{}",
                i.width(),
                i.height()
            ))
        )),
        Err(e) => out.push_str(&format!("probe failed: {e}\n")),
    }
    let frames = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let wake: Wake = Arc::new(|| {});
    match tuffcord::media::Player::open(file, [320, 180], 0.0, true, wake) {
        Ok(player) => {
            let start = Instant::now();
            let mut last = None;
            while start.elapsed() < Duration::from_secs(4) {
                if player.take_frame().is_some() {
                    frames.fetch_add(1, Ordering::Relaxed);
                }
                let st = player.status();
                if st.error.is_some() {
                    last = Some(st);
                    break;
                }
                last = Some(st);
                std::thread::sleep(Duration::from_millis(10));
            }
            out.push_str(&format!(
                "play: frames {} status {:?}\n",
                frames.load(Ordering::Relaxed),
                last
            ));
        }
        Err(e) => out.push_str(&format!("play failed: {e}\n")),
    }
    let _ = std::fs::write(report, out);
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--check-media" {
        check_media(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
        );
        return Ok(());
    }
    // Handed over by the previous copy when it restarts us after an update.
    let resume = updater::take_resume();
    let just_updated = std::env::args().any(|a| a == "--updated");
    // Backroom's settings folder becomes TUFFcord's (first run after the rename).
    tuffcord::settings::move_old_folders();
    tuffcord::crash::install();
    let last_run = tuffcord::crash::take_last_run();
    // A copy from before the rename (updated in place) takes its new file name.
    let renamed = updates::adopt_new_name();
    let settings = Settings::load();
    tuffcord::applog::info(format!(
        "TUFFcord {} started (log: {})",
        updates::CURRENT,
        tuffcord::applog::path().display()
    ));
    if let Some((old, new)) = renamed {
        tuffcord::applog::info(format!("Renamed {} to {}", old.display(), new.display()));
    }
    if let Some(p) = &last_run.interrupted {
        tuffcord::applog::warn(format!(
            "Last time, the app closed in the middle of playing {}",
            p.describe()
        ));
    }
    let options = eframe::NativeOptions {
        viewport: update_ui::main_viewport(&settings).with_icon(app_icon()),
        renderer: eframe::Renderer::Glow,
        vsync: true,
        ..Default::default()
    };
    let result = eframe::run_native(
        "TUFFcord",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                cc,
                settings,
                resume,
                just_updated,
                last_run,
            )))
        }),
    );
    // Closed normally: a video that was playing didn't cause anything.
    tuffcord::crash::playing_stopped();
    result
}

fn app_icon() -> egui::IconData {
    // Three amber level bars on a plum rounded square, drawn at 32x32.
    let n = 32usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = 8.0;
            let cx = fx.clamp(r, n as f32 - r);
            let cy = fy.clamp(r, n as f32 - r);
            let inside = (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r;
            let bar = |x0: f32, y0: f32, y1: f32| fx >= x0 && fx < x0 + 4.0 && fy >= y0 && fy < y1;
            let px = if !inside {
                [0, 0, 0, 0]
            } else if bar(7.5, 13.0, 20.0) || bar(14.0, 8.0, 24.0) || bar(20.5, 11.0, 22.0) {
                [244, 184, 96, 255]
            } else {
                [42, 33, 53, 255]
            };
            rgba[(y * n + x) * 4..(y * n + x) * 4 + 4].copy_from_slice(&px);
        }
    }
    egui::IconData {
        rgba,
        width: n as u32,
        height: n as u32,
    }
}

// ---------------------------------------------------------------- state

struct Session {
    me_id: u32,
    me_name: String,
    app_name: String,
    text_channels: Vec<String>,
    voice_channels: Vec<String>,
    max_per_voice: usize,
    history: BTreeMap<String, Vec<ChatMessage>>,
    voice_state: Vec<VoiceChannelState>,
    users: Vec<User>,
    /// Everyone with an account (servers with accounts), for the member list.
    members: Vec<Member>,
    /// Admins can add channels (newer servers).
    add_channels: bool,
    /// Admins can rename, delete and reorder channels (server 0.9.2+).
    edit_channels: bool,
    /// Older messages load on scrolling up (server 0.10+); see `history_ui`.
    older: history_ui::Older,
    /// Signed-in devices, invites, admin log (Settings > Account).
    security: security_ui::Security,
    /// Admins: the server's storage warning (text, urgent), and whether it was closed.
    storage_warning: Option<(String, bool)>,
    storage_hidden: bool,
}

impl Session {
    fn members(&self, channel: &str) -> &[proto::VoiceMember] {
        self.voice_state
            .iter()
            .find(|c| c.name == channel)
            .map(|c| c.members.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(PartialEq)]
enum Conn {
    Offline,
    Connecting,
    Online,
    Reconnecting,
}

struct Banner {
    text: String,
    error: bool,
    until: Option<Instant>,
}

struct VolumePop {
    name: String,
    /// Their account, for the admin buttons.
    account: Option<u32>,
    pos: egui::Pos2,
    opened_frame: u64,
}

#[derive(Default)]
struct UpdateState {
    checking: bool,
    checked: bool,
    latest: Option<Release>,
    error: Option<String>,
}

/// Looks for a newer release in the background. With `repeat`, keeps checking every few hours.
fn spawn_update_check(state: Arc<Mutex<UpdateState>>, wake: Wake, repeat: bool) {
    let _ = std::thread::Builder::new()
        .name("updates".into())
        .spawn(move || {
            if repeat {
                std::thread::sleep(Duration::from_secs(4));
            }
            loop {
                state.lock().checking = true;
                wake();
                let result = updates::check();
                {
                    let mut s = state.lock();
                    s.checking = false;
                    s.checked = true;
                    match result {
                        Ok(latest) => {
                            s.latest = latest;
                            s.error = None;
                        }
                        Err(e) => s.error = Some(e),
                    }
                }
                wake();
                if !repeat {
                    break;
                }
                std::thread::sleep(updates::CHECK_EVERY);
            }
        });
}

struct App {
    s: Settings,
    net: Net,
    mixer: Arc<Mutex<Mixer>>,
    ctl: Arc<VoiceControls>,
    keys: GlobalKeys,
    wake: Wake,
    audio_error: Arc<Mutex<Option<String>>>,
    input: Option<audio::Input>,
    output: Option<audio::Output>,
    input_failed: bool,
    output_failed: bool,
    audio_linger: Option<Instant>,
    devices: Option<DeviceList>,

    conn: Conn,
    login_error: Option<String>,
    session: Option<Session>,
    current_text: String,
    unread: BTreeSet<String>,
    voice_channel: Option<String>,
    joining: Option<String>,
    want_voice: Option<String>,
    /// The server said it's restarting (to update); explains the brief disconnect.
    server_restarting: Option<String>,
    ping: Option<u32>,
    composer: String,
    focus_composer: bool,
    banner: Option<Banner>,
    settings_open: bool,
    /// The category shown in the settings window.
    settings_page: settings_window::SettingsPage,
    pop: Option<VolumePop>,
    capturing: bool,
    mem_mb: Option<f64>,
    mem_checked: Option<Instant>,
    /// Space downloaded files use (Settings shows it), checked with `mem_mb`.
    cache_used: u64,
    update: Arc<Mutex<UpdateState>>,
    update_auto_started: bool,
    update_dismissed: Option<String>,
    att: Attachments,
    updater: Updater,
    update_failure_shown: Option<String>,
    emoji_open: bool,
    last_paste: Option<Instant>,
    v_press_seen: bool,
    emoji_anchor: egui::Rect,
    emoji_opened_frame: u64,
    acct: accounts::AccountUi,
    /// Color emoji pictures (borrowed while drawing messages).
    emoji: RefCell<chat_text::EmojiCache>,
    /// Message groups and their heights (borrowed while drawing messages).
    chat_layout: RefCell<chat::ChatLayout>,
    /// The video or audio playing in the chat.
    player: Option<attach_ui::Playing>,
    /// "Open this program?" waiting for an answer.
    confirm_open: Option<Attachment>,
    /// "Delete message?" waiting for an answer (admins).
    confirm_delete: Option<proto::ChatMessage>,
    new_channel: channels_ui::NewChannel,
    channel_edit: channels_ui::ChannelEdit,
    /// How the last run crashed, for the server's log once signed in.
    crash_report: Option<String>,
    /// Shown again once signed in (see `startup_notice`).
    startup_notice: Option<(String, bool, Option<u64>)>,
    /// Settings changed in a way that's saved once the mouse is let go
    /// (dragging a volume bar would otherwise save on every frame).
    settings_dirty: bool,
    /// The media volume to go back to when unmuting.
    unmute_level: f32,
    /// Emoji picker: the tab shown (a group, or RECENT) and what's typed in its search.
    emoji_tab: u8,
    emoji_search: String,
    /// Middle-click auto scroll in the chat.
    autoscroll: autoscroll::AutoScroll,
}

/// The emoji picker's "recently used" tab.
const RECENT: u8 = 255;

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        s: Settings,
        resume: Option<updater::Resume>,
        just_updated: bool,
        last_run: tuffcord::crash::LastRun,
    ) -> Self {
        theme::apply(&cc.egui_ctx, &s.theme);
        emoji::install_font(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let ctl = Arc::new(VoiceControls::default());
        let net = Net::spawn(mixer.clone(), wake.clone());
        let keys = GlobalKeys::start(ctl.clone(), wake.clone());
        let audio_error = Arc::new(Mutex::new(None));
        let att = Attachments::new(wake.clone());
        let mut app = App {
            s,
            net,
            mixer,
            ctl,
            keys,
            wake,
            audio_error,
            input: None,
            output: None,
            input_failed: false,
            output_failed: false,
            audio_linger: None,
            devices: None,
            conn: Conn::Offline,
            login_error: None,
            session: None,
            current_text: String::new(),
            unread: BTreeSet::new(),
            voice_channel: None,
            joining: None,
            want_voice: None,
            server_restarting: None,
            ping: None,
            composer: String::new(),
            focus_composer: false,
            banner: None,
            settings_open: false,
            settings_page: Default::default(),
            pop: None,
            capturing: false,
            mem_mb: None,
            mem_checked: None,
            cache_used: 0,
            update: Arc::new(Mutex::new(UpdateState::default())),
            update_auto_started: false,
            update_dismissed: None,
            att,
            updater: Updater::default(),
            update_failure_shown: None,
            emoji_open: false,
            last_paste: None,
            v_press_seen: false,
            emoji_anchor: egui::Rect::NOTHING,
            emoji_opened_frame: 0,
            acct: accounts::AccountUi::default(),
            emoji: RefCell::new(chat_text::EmojiCache::default()),
            chat_layout: RefCell::default(),
            player: None,
            confirm_open: None,
            confirm_delete: None,
            new_channel: Default::default(),
            channel_edit: Default::default(),
            autoscroll: Default::default(),
            crash_report: None,
            startup_notice: None,
            settings_dirty: false,
            unmute_level: tuffcord::settings::DEFAULT_MEDIA_LEVEL,
            emoji_tab: RECENT,
            emoji_search: String::new(),
        };
        app.att.cache_budget = app.s.cache_bytes();
        let budget = app.att.cache_budget;
        std::thread::spawn(move || tuffcord::files::prune_cache(budget));
        app.apply_voice_flags();
        if app.s.check_updates {
            app.update_auto_started = true;
            spawn_update_check(app.update.clone(), app.wake.clone(), true);
        }
        updater::cleanup_leftovers();
        // Someone who used the group password before accounts: it's what
        // "Create account" needs, so have it ready.
        app.acct.invite = app.s.password.clone();
        if let Some(r) = resume.filter(|r| !r.server.is_empty() && !r.name.is_empty()) {
            // Restarted by an update: sign back in and go back to where we were.
            app.s.server = r.server;
            app.s.name = r.name;
            app.want_voice = r.voice;
            if let Some(t) = r.text_channel {
                app.current_text = t;
            }
            // Copies up to 0.7.1 hand over an empty sign-in when they were signed in
            // with the saved key (the usual case): use the saved one then.
            let (token, password) = if r.token.is_empty() && r.password.is_empty() {
                (app.s.token.clone(), app.s.password.clone())
            } else {
                (r.token, r.password)
            };
            if !app.connect_saved(token, password) {
                app.want_voice = None;
            }
        } else if app.s.auto_connect && app.s.remember {
            let (token, group) = (app.s.token.clone(), app.s.password.clone());
            app.connect_saved(token, group);
        }
        if just_updated {
            app.startup_notice(
                format!("Updated to TUFFcord {}.", updates::CURRENT),
                false,
                Some(8),
            );
        }
        app.after_bad_exit(last_run);
        app
    }

    /// A message for the top of the chat that also survives signing in
    /// (which clears connection messages).
    fn startup_notice(&mut self, text: impl Into<String>, error: bool, secs: Option<u64>) {
        let text = text.into();
        self.show_banner(text.clone(), error, secs);
        self.startup_notice = Some((text, error, secs));
    }

    /// The previous run crashed, or ended while playing something: say so,
    /// tell the server's log once signed in, and avoid doing it again.
    fn after_bad_exit(&mut self, last: tuffcord::crash::LastRun) {
        let mut report = last.crash.clone().map(|c| format!("The app crashed: {c}"));
        if let Some(p) = last.interrupted {
            let why = if last.crash.is_some() {
                "crashed"
            } else {
                "closed unexpectedly"
            };
            let msg = if p.gpu {
                self.s.video_gpu = false;
                format!(
                    "TUFFcord {why} while playing {}. Videos now play without the graphics card (you can change that in Settings).",
                    p.name
                )
            } else {
                if !self.s.play_outside.contains(&p.id) {
                    self.s.play_outside.insert(0, p.id.clone());
                    self.s.play_outside.truncate(50);
                }
                format!(
                    "TUFFcord {why} while playing {}, so that file will open in your video player from now on.",
                    p.name
                )
            };
            self.s.save();
            self.startup_notice(msg, true, None);
            if report.is_none() {
                report = Some(format!(
                    "The app closed unexpectedly while playing {}",
                    p.describe()
                ));
            }
        } else if last.crash.is_some() {
            self.startup_notice(
                "TUFFcord crashed last time. The reason is in the log (Settings → Open log file) and was sent to the server's log.",
                true,
                Some(12),
            );
        }
        self.crash_report = report;
    }

    // ------------------------------------------------------------ actions

    fn sign_out(&mut self) {
        self.forget_sign_in();
        self.leave_voice_quietly();
        self.want_voice = None;
        self.net.disconnect();
        self.session = None;
        self.conn = Conn::Offline;
        self.s.auto_connect = false;
        if !self.s.remember {
            self.s.password.clear();
        }
        self.s.save();
        self.settings_open = false;
        self.banner = None;
    }

    fn apply_voice_flags(&mut self) {
        self.ctl.muted.store(self.s.muted, Ordering::Relaxed);
        self.ctl.deafened.store(self.s.deafened, Ordering::Relaxed);
        self.ctl
            .push_to_talk
            .store(self.s.push_to_talk, Ordering::Relaxed);
        self.ctl
            .noise_suppression
            .store(self.s.noise_suppression, Ordering::Relaxed);
        self.ctl.set_threshold_db(self.s.threshold_db);
        self.keys.set_key(self.s.ptt_key);
        self.keys.set_active(self.s.push_to_talk);
        self.mixer.lock().deafened = self.s.deafened;
    }

    fn sound(&self, kind: &str) {
        if self.s.sounds && self.output.is_some() {
            self.mixer.lock().play_system(&chime(kind));
        }
    }

    fn send_status(&self) {
        if self.voice_channel.is_some() {
            self.net.send(ClientMsg::Status {
                muted: self.s.muted,
                deafened: self.s.deafened,
            });
        }
    }

    fn toggle_mute(&mut self) {
        if self.s.deafened {
            // Unmuting while deafened also undeafens.
            self.s.deafened = false;
            self.s.muted = false;
        } else {
            self.s.muted = !self.s.muted;
        }
        self.apply_voice_flags();
        self.send_status();
        self.sound(if self.s.muted { "mute" } else { "unmute" });
        self.s.save();
    }

    fn toggle_deafen(&mut self) {
        self.s.deafened = !self.s.deafened;
        self.apply_voice_flags();
        self.send_status();
        self.sound(if self.s.deafened { "mute" } else { "unmute" });
        self.s.save();
    }

    fn join_voice(&mut self, channel: String) {
        if self.conn != Conn::Online
            || self.voice_channel.as_deref() == Some(channel.as_str())
            || self.joining.is_some()
        {
            return;
        }
        self.input_failed = false;
        self.output_failed = false;
        self.joining = Some(channel.clone());
        self.net.send(ClientMsg::JoinVoice {
            channel,
            muted: self.s.muted,
            deafened: self.s.deafened,
        });
    }

    fn leave_voice(&mut self) {
        if self.voice_channel.is_none() {
            return;
        }
        self.net.send(ClientMsg::LeaveVoice);
        self.sound("leave");
        self.leave_voice_quietly();
        self.audio_linger = Some(Instant::now() + Duration::from_millis(500));
    }

    fn leave_voice_quietly(&mut self) {
        self.voice_channel = None;
        self.joining = None;
        self.ctl.in_voice.store(false, Ordering::Relaxed);
        self.ctl.talking.store(false, Ordering::Relaxed);
        self.mixer.lock().clear();
        self.ping = None;
    }

    fn apply_gains(&self) {
        let Some(sess) = &self.session else { return };
        let mut m = self.mixer.lock();
        for ch in &sess.voice_state {
            for mem in &ch.members {
                m.set_gain(mem.id, self.s.gain(&mem.name));
            }
        }
    }

    fn show_banner(&mut self, text: impl Into<String>, error: bool, secs: Option<u64>) {
        let text = text.into();
        // Everything the app tells the person goes in its log too.
        if self.banner.as_ref().is_none_or(|b| b.text != text) {
            if error {
                tuffcord::applog::warn(format!("Shown: {text}"));
            } else {
                tuffcord::applog::info(format!("Shown: {text}"));
            }
        }
        self.banner = Some(Banner {
            text,
            error,
            until: secs.map(|s| Instant::now() + Duration::from_secs(s)),
        });
    }

    // ------------------------------------------------------------ audio streams

    /// Mic and speakers run only while you're in voice (or testing in settings).
    fn sync_audio(&mut self) {
        let device_error = self.audio_error.lock().take();
        if let Some(err) = device_error {
            // A device went away: drop the streams so they reopen (on the default device if needed).
            self.input = None;
            self.output = None;
            self.show_banner(err.clone(), true, Some(8));
            self.net.send(ClientMsg::Report {
                kind: if err.starts_with("Microphone") {
                    "mic".into()
                } else {
                    "speaker".into()
                },
                message: err,
            });
        }
        let lingering = self.audio_linger.is_some_and(|t| Instant::now() < t);
        let want = self.voice_channel.is_some()
            || self.joining.is_some()
            || self.settings_open
            || lingering;
        if !want {
            self.input = None;
            self.output = None;
            return;
        }
        if self.input.is_none() && !self.input_failed {
            let hook = self.error_hook();
            match audio::start_input(
                self.s.input_device.as_deref(),
                self.ctl.clone(),
                self.net.voice_sender(),
                self.wake.clone(),
                hook,
            ) {
                Ok(i) => self.input = Some(i),
                Err(e) => {
                    self.input_failed = true;
                    self.show_banner(e.clone(), true, Some(10));
                    self.net.send(ClientMsg::Report {
                        kind: "mic".into(),
                        message: e,
                    });
                }
            }
        }
        if self.output.is_none() && !self.output_failed {
            let hook = self.error_hook();
            match audio::start_output(self.s.output_device.as_deref(), self.mixer.clone(), hook) {
                Ok(o) => self.output = Some(o),
                Err(e) => {
                    self.output_failed = true;
                    self.show_banner(e.clone(), true, Some(10));
                    self.net.send(ClientMsg::Report {
                        kind: "speaker".into(),
                        message: e,
                    });
                }
            }
        }
    }

    fn error_hook(&self) -> audio::ErrorHook {
        let slot = self.audio_error.clone();
        let wake = self.wake.clone();
        Arc::new(move |e| {
            *slot.lock() = Some(e);
            wake();
        })
    }

    // ------------------------------------------------------------ input outside widgets

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.capturing {
            if !GlobalKeys::is_global() {
                let pressed = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Key {
                            key, pressed: true, ..
                        } => Some(*key),
                        _ => None,
                    })
                });
                if let Some(k) = pressed {
                    if k == Key::Escape {
                        self.keys.cancel_capture();
                        self.capturing = false;
                    } else if let Some(vk) = keys::vk_from_egui(k) {
                        self.keys.finish_capture(vk);
                    }
                }
            }
            if let Some(vk) = self.keys.take_captured() {
                if vk != 0x1B {
                    self.s.ptt_key = vk;
                    self.apply_voice_flags();
                    self.s.save();
                }
                self.capturing = false;
            } else if !self.keys.is_capturing() && GlobalKeys::is_global() {
                self.capturing = false;
            }
        }
        // Without a global key hook, push-to-talk works while this window has focus.
        if self.s.push_to_talk && !GlobalKeys::is_global() {
            let typing = ctx.memory(|m| m.focused().is_some());
            let held = !typing
                && ctx.input(|i| {
                    egui::Key::ALL
                        .iter()
                        .any(|k| keys::vk_from_egui(*k) == Some(self.s.ptt_key) && i.key_down(*k))
                });
            if held != self.ctl.ptt_held.load(Ordering::Relaxed) {
                self.ctl.ptt_held.store(held, Ordering::Relaxed);
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.settings_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.settings_dirty = false;
            self.s.save();
        }
        self.remember_window(ctx);
        self.handle_events(ctx);
        self.handle_keys(ctx);
        self.handle_image_input(ctx);
        match self.updater.phase() {
            Phase::Ready { exe, .. } => self.restart_into_update(exe),
            Phase::Failed(msg) if self.update_failure_shown.as_deref() != Some(msg.as_str()) => {
                // The bar only has room for a short note; show the whole explanation once.
                self.show_banner(format!("The update didn't finish: {msg}"), true, Some(15));
                self.update_failure_shown = Some(msg);
            }
            Phase::Idle | Phase::Downloading { .. } => self.update_failure_shown = None,
            _ => {}
        }
        self.sync_audio();

        if self.session.is_some() && self.conn != Conn::Offline {
            self.main_screen(ctx);
            self.account_dialogs(ctx);
        } else {
            self.login_screen(ctx);
        }
        self.banner(ctx);

        // Redraw only when something can change on its own.
        let in_voice = self.voice_channel.is_some();
        let someone_talking = in_voice
            && (self.ctl.talking.load(Ordering::Relaxed)
                || !self.mixer.lock().speaking().is_empty());
        if self.settings_open {
            ctx.request_repaint_after(Duration::from_millis(60));
        } else if someone_talking {
            // Starting and stopping already wake the UI (mic callback, voice
            // receiver); this only catches someone whose stop never arrived.
            ctx.request_repaint_after(Duration::from_millis(1000));
        } else if let Some(t) = self.banner.as_ref().and_then(|b| b.until) {
            ctx.request_repaint_after(
                t.saturating_duration_since(Instant::now()) + Duration::from_millis(20),
            );
        } else if self.audio_linger.is_some_and(|t| Instant::now() < t) {
            ctx.request_repaint_after(Duration::from_millis(550));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.s.save();
        self.net.disconnect();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [
            pal().bg_deep.r() as f32 / 255.0,
            pal().bg_deep.g() as f32 / 255.0,
            pal().bg_deep.b() as f32 / 255.0,
            1.0,
        ]
    }
}
