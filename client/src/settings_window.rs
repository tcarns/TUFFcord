//! The settings window: categories on the left, one function per section on the right.

use crate::theme::{self, pal};
use crate::widgets::theme_swatch;
use crate::{spawn_update_check, App};
use eframe::egui::{self, Align2, CornerRadius, Frame, Margin, RichText, Sense, Stroke};
use proto::update::{self as updates};
use std::time::{Duration, Instant};
use tuffcord::audio::{self, DeviceList};
use tuffcord::images::size_label;
use tuffcord::keys::{self, GlobalKeys};
use tuffcord::settings::MIN_CACHE_MB;
use tuffcord::voice::chime;

/// The categories down the left side of the settings window.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsPage {
    #[default]
    Audio,
    Interface,
    Account,
    System,
}

impl SettingsPage {
    const ALL: [SettingsPage; 4] = [Self::Audio, Self::Interface, Self::Account, Self::System];

    fn label(self) -> &'static str {
        match self {
            Self::Audio => "Voice and audio",
            Self::Interface => "Appearance and videos",
            Self::Account => "Account",
            Self::System => "Updates and about",
        }
    }
}

/// Width of the category list and of the settings beside it.
const NAV_W: f32 = 170.0;
const PAGE_W: f32 = 400.0;

impl App {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        if self.devices.is_none() {
            self.devices = Some(audio::list_devices());
        }
        if self
            .mem_checked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.mem_mb = memory_stats::memory_stats().map(|m| m.physical_mem as f64 / 1_048_576.0);
            self.mem_checked = Some(Instant::now());
            self.cache_used = tuffcord::files::cache_used();
        }
        let mut open = true;
        let mut reopen_input = false;
        let mut reopen_output = false;
        let mut flags_changed = false;
        let mut sign_out = false;
        egui::Window::new(RichText::new("Settings").strong().size(17.0))
            .collapsible(false)
            .resizable(false)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(ctx.screen_rect().center())
            .open(&mut open)
            .frame(
                Frame::window(&ctx.style())
                    .fill(pal().bg)
                    .inner_margin(Margin::same(22)),
            )
            .show(ctx, |ui| {
                // The same height on every page, so the window doesn't jump around.
                let h = (ctx.screen_rect().height() - 140.0).clamp(200.0, 470.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(NAV_W);
                        self.settings_nav(ui);
                    });
                    ui.add_space(8.0);
                    let (line, _) = ui.allocate_exact_size(egui::vec2(1.0, h), Sense::hover());
                    ui.painter()
                        .rect_filled(line, CornerRadius::ZERO, pal().line);
                    ui.add_space(14.0);
                    egui::ScrollArea::vertical()
                        .max_height(h)
                        .min_scrolled_height(h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(PAGE_W);
                                match self.settings_page {
                                    SettingsPage::Audio => {
                                        let devices = self.devices.clone().unwrap_or_default();
                                        reopen_input = self.microphone_section(ui, &devices);
                                        ui.add_space(8.0);
                                        reopen_output = self.speakers_section(ui, &devices);
                                        divider(ui);
                                        flags_changed = self.voice_section(ui);
                                        divider(ui);
                                        self.media_volume_section(ui);
                                    }
                                    SettingsPage::Interface => {
                                        self.theme_section(ui);
                                        divider(ui);
                                        self.videos_section(ui);
                                        divider(ui);
                                        self.downloads_section(ui);
                                    }
                                    SettingsPage::Account => sign_out = self.account_section(ui),
                                    SettingsPage::System => self.updates_section(ui),
                                }
                            })
                        });
                });
            });
        if reopen_input {
            self.input = None;
            self.input_failed = false;
        }
        if reopen_output {
            self.output = None;
            self.output_failed = false;
        }
        if flags_changed {
            self.apply_voice_flags();
        }
        if reopen_input || reopen_output || flags_changed {
            self.s.save();
        }
        if sign_out {
            self.sign_out();
            return;
        }
        if !open {
            self.settings_open = false;
            self.devices = None;
            self.capturing = false;
            self.keys.cancel_capture();
            self.s.save();
        }
    }

    /// The category list on the left: click one to show its settings.
    fn settings_nav(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 2.0;
        for page in SettingsPage::ALL {
            let selected = self.settings_page == page;
            let text = RichText::new(page.label()).size(14.0).color(if selected {
                pal().text
            } else {
                pal().muted
            });
            let btn = egui::Button::new(text)
                .fill(if selected {
                    pal().raised_2
                } else {
                    egui::Color32::TRANSPARENT
                })
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(6))
                .min_size(egui::vec2(NAV_W, 34.0));
            if ui.add(btn).clicked() && !selected {
                self.settings_page = page;
                // Leaving the page with the push-to-talk key picker open.
                if self.capturing {
                    self.capturing = false;
                    self.keys.cancel_capture();
                }
            }
        }
    }

    /// Microphone choice, level meter and test hint. True when the input device changed.
    fn microphone_section(&mut self, ui: &mut egui::Ui, devices: &DeviceList) -> bool {
        let mut reopen_input = false;
        heading(ui, "Microphone");
        let current = self
            .s
            .input_device
            .clone()
            .unwrap_or_else(|| "System default".into());
        egui::ComboBox::from_id_salt("input")
            .width(400.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(&mut self.s.input_device, None, "System default")
                    .changed()
                {
                    reopen_input = true;
                }
                for d in &devices.inputs {
                    if ui
                        .selectable_value(&mut self.s.input_device, Some(d.clone()), d)
                        .changed()
                    {
                        reopen_input = true;
                    }
                }
            });
        // Level meter, with the voice activation threshold marked.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(400.0, 10.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(5), pal().bg_deep);
        let to_x = |db: f32| rect.left() + rect.width() * ((db + 70.0) / 70.0).clamp(0.0, 1.0);
        let level = self.ctl.level_db();
        let above = level > self.s.threshold_db;
        let fill = if !self.s.push_to_talk && above {
            pal().teal
        } else {
            theme::mix(pal().teal, pal().bg_deep, 0.45)
        };
        p.rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(to_x(level), rect.max.y)),
            CornerRadius::same(5),
            fill,
        );
        if !self.s.push_to_talk {
            let x = to_x(self.s.threshold_db);
            p.line_segment(
                [
                    egui::pos2(x, rect.top() - 3.0),
                    egui::pos2(x, rect.bottom() + 3.0),
                ],
                Stroke::new(2.0_f32, pal().accent),
            );
        }
        let hint = if self.input.is_some() {
            if self.s.muted || self.s.deafened {
                "You're muted, but the meter still shows your mic for testing."
            } else {
                "Talk to test your mic."
            }
        } else {
            "Microphone isn't available."
        };
        ui.label(RichText::new(hint).size(12.5).color(pal().faint));
        reopen_input
    }

    /// Speaker choice and test sound. True when the output device changed.
    fn speakers_section(&mut self, ui: &mut egui::Ui, devices: &DeviceList) -> bool {
        let mut reopen_output = false;
        heading(ui, "Speakers or headphones");
        let current = self
            .s
            .output_device
            .clone()
            .unwrap_or_else(|| "System default".into());
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("output")
                .width(290.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut self.s.output_device, None, "System default")
                        .changed()
                    {
                        reopen_output = true;
                    }
                    for d in &devices.outputs {
                        if ui
                            .selectable_value(&mut self.s.output_device, Some(d.clone()), d)
                            .changed()
                        {
                            reopen_output = true;
                        }
                    }
                });
            if ui.button("Test sound").clicked() {
                self.mixer.lock().play_system(&chime("join"));
            }
        });
        reopen_output
    }

    /// When to send voice (voice activation or push to talk), noise suppression, sounds. True when a voice flag changed.
    fn voice_section(&mut self, ui: &mut egui::Ui) -> bool {
        let mut flags_changed = false;
        heading(ui, "When to send your voice");
        ui.horizontal(|ui| {
            flags_changed |= ui
                .radio_value(&mut self.s.push_to_talk, false, "When I talk")
                .changed();
            flags_changed |= ui
                .radio_value(&mut self.s.push_to_talk, true, "Push to talk")
                .changed();
        });
        if self.s.push_to_talk {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Key").color(pal().muted));
                let label = if self.capturing {
                    "Press a key or mouse button…".to_string()
                } else {
                    keys::key_name(self.s.ptt_key)
                };
                let btn = egui::Button::new(RichText::new(label).color(if self.capturing {
                    pal().accent_ink
                } else {
                    pal().text
                }))
                .fill(if self.capturing {
                    pal().accent
                } else {
                    pal().raised_2
                })
                .min_size(egui::vec2(220.0, 30.0));
                if ui.add(btn).clicked() && !self.capturing {
                    self.capturing = true;
                    self.keys.begin_capture();
                }
            });
            let note = if GlobalKeys::is_global() {
                "Works even while another app or a game is in front. Esc cancels."
            } else {
                "Works while this window is in front."
            };
            ui.label(RichText::new(note).size(12.5).color(pal().faint));
        } else {
            ui.label(
                RichText::new("Sensitivity: your mic opens when it passes the amber line.")
                    .size(12.5)
                    .color(pal().faint),
            );
            if ui
                .add(egui::Slider::new(&mut self.s.threshold_db, -70.0..=-10.0).show_value(false))
                .changed()
            {
                flags_changed = true;
            }
        }
        ui.add_space(6.0);
        flags_changed |= ui
            .checkbox(
                &mut self.s.noise_suppression,
                "Noise suppression (removes fans, typing, background hum)",
            )
            .changed();
        ui.checkbox(&mut self.s.sounds, "Join, leave and message sounds");
        flags_changed
    }

    /// The volume each video or audio clip in the chat starts at.
    fn media_volume_section(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Default video and audio volume");
        let mut pct = (self.s.media_default * 100.0).round();
        let resp = ui.add(
            egui::Slider::new(&mut pct, 0.0..=100.0)
                .suffix("%")
                .fixed_decimals(0)
                .step_by(1.0),
        );
        if resp.changed() {
            self.s.media_default = pct / 100.0;
            // Nothing playing: the speaker buttons in the chat show the new level.
            if self.player.is_none() {
                self.s.media_level = self.s.media_default;
            }
            // Saved once the slider is let go.
            self.settings_dirty = true;
        }
        ui.label(
            RichText::new("Every video or audio file starts at this volume. The speaker button on a player changes it for that one only.")
                .size(12.5)
                .color(pal().faint),
        );
    }

    /// Video decoding and the files that open in the system's player.
    fn videos_section(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Videos");
        if tuffcord::media::supported()
            && ui
                .checkbox(&mut self.s.video_gpu, "Use the graphics card for videos")
                .on_hover_text("Lighter on the processor. Turned off by itself if TUFFcord ever closes in the middle of a video.")
                .changed()
        {
            self.s.save();
        }
        // Always shown, greyed out until TUFFcord has sent a file to the
        // system's player after closing in the middle of it.
        let outside = self.s.play_outside.len();
        let label = if outside > 0 {
            format!("Play videos here again ({outside} open in your video player)")
        } else {
            "Play videos here again".to_string()
        };
        let explain = "If TUFFcord ever closes in the middle of playing a video, that video opens in your computer's video player from then on, so it can't close TUFFcord again. This button plays those videos inside TUFFcord again. It's greyed out while there are none.";
        if ui
            .add_enabled(outside > 0, egui::Button::new(label))
            .on_hover_text(explain)
            .on_disabled_hover_text(explain)
            .clicked()
        {
            self.s.play_outside.clear();
            self.s.save();
        }
    }

    /// Clips downloaded ahead of time, and the space downloaded files may use.
    fn downloads_section(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Downloads");
        ui.label("Download videos and audio ahead of time, up to");
        let resp = ui.add(
            egui::Slider::new(&mut self.s.preload_mb, 0..=200)
                .step_by(5.0)
                .custom_formatter(|v, _| {
                    if v == 0.0 {
                        "Off".into()
                    } else {
                        format!("{v:.0} MB")
                    }
                }),
        );
        if resp.changed() {
            self.settings_dirty = true;
        }
        ui.label(
            RichText::new("Clips this size or smaller download as soon as they show up in the chat, so they play right away. Larger ones download when you press Play.")
                .size(12.5)
                .color(pal().faint),
        );
        ui.add_space(10.0);
        ui.label("Space for downloaded files");
        let mut gb = self.s.cache_mb as f32 / 1024.0;
        let min = MIN_CACHE_MB as f32 / 1024.0;
        let resp = ui.add(
            egui::Slider::new(&mut gb, min..=20.0)
                .step_by(0.25)
                .suffix(" GB")
                .fixed_decimals(2),
        );
        if resp.changed() {
            self.s.cache_mb = ((gb * 1024.0).round() as u32).max(MIN_CACHE_MB);
            self.att.cache_budget = self.s.cache_bytes();
            self.settings_dirty = true;
        }
        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
            // Smaller than what's stored: remove the oldest files now.
            let budget = self.att.cache_budget;
            std::thread::spawn(move || tuffcord::files::prune_cache(budget));
            self.mem_checked = None;
        }
        ui.label(
            RichText::new(format!(
                "In use: {}. The oldest files are removed when it's full.",
                size_label(self.cache_used)
            ))
            .size(12.5)
            .color(pal().faint),
        );
    }

    /// Theme swatches.
    fn theme_section(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Theme");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
            for t in theme::THEMES {
                let selected = self.s.theme == t.id;
                if theme_swatch(ui, &t, selected).clicked() && !selected {
                    self.s.theme = t.id.to_string();
                    theme::apply(ui.ctx(), t.id);
                    self.s.save();
                }
            }
        });
    }

    /// Update checks, the log file, memory use and credits.
    fn updates_section(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Updates");
        let (checking, latest, error, checked) = {
            let u = self.update.lock();
            (u.checking, u.latest.clone(), u.error.clone(), u.checked)
        };
        {
            let status = if checking {
                "Checking…".to_string()
            } else if let Some(r) = &latest {
                format!("TUFFcord {} is available.", r.version)
            } else if let Some(e) = &error {
                format!("Couldn't check: {e}.")
            } else if checked {
                "You're up to date.".to_string()
            } else {
                String::new()
            };
            // Wrapped: this window fits its content, so one long line
            // (like an error from GitHub) would stretch it across the screen.
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Version {}. {status}", updates::CURRENT)).size(13.0),
                )
                .wrap(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !checking && !self.updater.busy(),
                    egui::Button::new("Check now"),
                )
                .clicked()
            {
                spawn_update_check(self.update.clone(), self.wake.clone(), false);
            }
            if let Some(r) = &latest {
                self.update_controls(ui, r);
            }
        });
        if ui
            .checkbox(&mut self.s.check_updates, "Check for updates automatically")
            .changed()
        {
            if self.s.check_updates && !self.update_auto_started {
                self.update_auto_started = true;
                spawn_update_check(self.update.clone(), self.wake.clone(), true);
            }
            self.s.save();
        }
        if ui
            .button("Open log file")
            .on_hover_text(tuffcord::applog::path().display().to_string())
            .clicked()
        {
            let _ = tuffcord::files::open_path(&tuffcord::applog::path());
        }
        if let Some(mb) = self.mem_mb {
            ui.label(
                RichText::new(format!("TUFFcord is using {mb:.0} MB of memory."))
                    .size(12.5)
                    .color(pal().faint),
            );
        }
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Emoji: Twemoji by Twitter, Inc. and contributors (CC-BY 4.0). Emoji names: emojibase (MIT).",
                )
                .size(11.5)
                .color(pal().faint),
            )
            .wrap(),
        );
    }

    /// Account settings and the sign-out button. True when sign out was clicked.
    fn account_section(&mut self, ui: &mut egui::Ui) -> bool {
        let mut sign_out = false;
        self.account_settings(ui);
        self.security_settings(ui);
        ui.add_space(6.0);
        let signout =
            egui::Button::new(RichText::new("Sign out").color(pal().red)).fill(pal().raised_2);
        if ui.add(signout).clicked() {
            sign_out = true;
        }
        sign_out
    }
}

/// A section's title.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).strong().color(pal().muted));
}

/// The line between settings sections.
fn divider(ui: &mut egui::Ui) {
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(4.0);
}
