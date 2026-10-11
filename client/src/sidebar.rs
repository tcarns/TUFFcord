//! The main screen layout and the left sidebar: channel lists, voice panel, own user row.

use crate::channels_ui::{self, ListActions};
use crate::theme::pal;
use crate::widgets::{bar_button, icon_button, paint_avatar, panel_edge};
use crate::{members, App, VolumePop};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense,
    Stroke,
};
use proto::ClientMsg;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use tuffcord::keys::{self};
use tuffcord::settings::{MEMBERS_WIDTH, SIDEBAR_WIDTH};

impl App {
    pub(crate) fn main_screen(&mut self, ctx: &egui::Context) {
        let speaking: HashSet<u32> = {
            let mut s: HashSet<u32> = self.mixer.lock().speaking().into_iter().collect();
            if let Some(sess) = &self.session {
                if self.ctl.talking.load(Ordering::Relaxed) {
                    s.insert(sess.me_id);
                }
            }
            s
        };
        let side = self.sidebar(ctx, &speaking);
        let members = self.member_panel(ctx, &speaking);
        self.chat(ctx);
        self.panel_edges(ctx, side, members);
        self.confirm_open_dialog(ctx);
        self.confirm_delete_dialog(ctx);
        self.confirm_channel_delete(ctx);
        self.volume_popup(ctx);
        self.emoji_picker(ctx);
        self.image_viewer(ctx);
        self.drop_overlay(ctx);
        if self.settings_open {
            self.settings_window(ctx);
        }
    }

    /// Dragging the edges of the sidebar and the member list (widths saved
    /// in this app's settings only). The chat keeps at least `MIN_CHAT`.
    fn panel_edges(&mut self, ctx: &egui::Context, side: egui::Rect, members: Option<egui::Rect>) {
        const MIN_CHAT: f32 = 360.0;
        let screen = ctx.screen_rect().width();
        let members_w = members.map_or(0.0, |r| r.width());
        let (min, _, max) = SIDEBAR_WIDTH;
        let max = max.min(screen - members_w - MIN_CHAT);
        let mut done = panel_edge(ctx, side, true, &mut self.s.sidebar_width, (min, max));
        if let Some(r) = members {
            let (min, _, max) = MEMBERS_WIDTH;
            let max = max.min(screen - side.width() - MIN_CHAT);
            done |= panel_edge(ctx, r, false, &mut self.s.members_width, (min, max));
        }
        if done {
            self.s.save();
        }
    }

    pub(crate) fn sidebar(&mut self, ctx: &egui::Context, speaking: &HashSet<u32>) -> egui::Rect {
        let (min, _, max) = SIDEBAR_WIDTH;
        egui::SidePanel::left("sidebar")
            .exact_width(self.s.sidebar_width.clamp(min, max))
            .resizable(false)
            .frame(Frame::new().fill(pal().bg_deep))
            .show(ctx, |ui| {
                // Bottom: voice connection + me.
                egui::TopBottomPanel::bottom("me")
                    .frame(Frame::new().fill(pal().me_bg).inner_margin(Margin {
                        left: 12,
                        right: 8,
                        top: 8,
                        bottom: 10,
                    }))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        if self.voice_channel.is_some() || self.joining.is_some() {
                            self.voice_panel(ui);
                            ui.add_space(6.0);
                        }
                        self.me_row(ui, speaking);
                    });
                egui::TopBottomPanel::top("brand")
                    .frame(Frame::new().inner_margin(Margin {
                        left: 16,
                        right: 12,
                        top: 14,
                        bottom: 4,
                    }))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        let name = self
                            .session
                            .as_ref()
                            .map(|s| s.app_name.clone())
                            .unwrap_or_default();
                        ui.label(RichText::new(name).size(19.0).strong());
                    });
                egui::CentralPanel::default()
                    .frame(Frame::new().inner_margin(Margin::symmetric(8, 0)))
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(ui, |ui| self.channel_lists(ui, speaking));
                    });
            })
            .response
            .rect
    }

    pub(crate) fn row(
        ui: &mut egui::Ui,
        height: f32,
        selected: bool,
    ) -> (egui::Rect, egui::Response) {
        Self::row_sense(ui, height, selected, Sense::click())
    }

    /// A row that can also be dragged (`Sense::click_and_drag`), for channels admins reorder.
    pub(crate) fn row_sense(
        ui: &mut egui::Ui,
        height: f32,
        selected: bool,
        sense: Sense,
    ) -> (egui::Rect, egui::Response) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), sense);
        let fill = if selected {
            pal().raised_2
        } else if resp.hovered() || resp.dragged() {
            pal().raised
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        (rect, resp)
    }

    pub(crate) fn channel_lists(&mut self, ui: &mut egui::Ui, speaking: &HashSet<u32>) {
        let admin = self.is_admin();
        let Some(sess) = &self.session else { return };
        let can_add = admin && sess.add_channels;
        let can_edit = admin && sess.edit_channels;
        let mut act = ListActions::default();
        act.create = channels_ui::header(ui, false, can_add, &mut self.new_channel);
        self.text_list(ui, can_edit, &mut act);
        act.create = act.create.or(channels_ui::header(
            ui,
            true,
            can_add,
            &mut self.new_channel,
        ));
        self.voice_list(ui, speaking, can_edit, &mut act);
        ui.add_space(12.0);

        if let Some(ch) = act.select_text {
            self.select_text_channel(ch);
        }
        if let Some((name, voice)) = act.create {
            self.net.send(ClientMsg::CreateChannel { name, voice });
        }
        if let Some((voice, name, to)) = act.rename {
            self.net.send(ClientMsg::RenameChannel { name, to, voice });
        }
        if let Some((voice, name, index)) = act.move_to {
            self.move_channel(voice, name, index);
        }
        if let Some(ch) = act.join {
            self.join_voice(ch);
        }
        if let Some(p) = act.open_pop {
            self.pop = Some(p);
        }
    }

    fn text_list(&mut self, ui: &mut egui::Ui, can_edit: bool, act: &mut ListActions) {
        let Some(sess) = &self.session else { return };
        let sense = if can_edit {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let mut rows = Vec::new();
        for (i, ch) in sess.text_channels.iter().enumerate() {
            if let Some(r) = channels_ui::rename_box(ui, &mut self.channel_edit, false, ch, act) {
                rows.push(r);
                continue;
            }
            let selected = *ch == self.current_text;
            let unread = self.unread.contains(ch);
            let (rect, resp) = Self::row_sense(ui, 30.0, selected, sense);
            rows.push(rect);
            let p = ui.painter();
            p.text(
                rect.left_center() + egui::vec2(12.0, 0.0),
                Align2::LEFT_CENTER,
                "#",
                FontId::proportional(16.0),
                pal().faint,
            );
            let color = if selected || unread {
                pal().text
            } else {
                pal().muted
            };
            p.text(
                rect.left_center() + egui::vec2(32.0, 0.0),
                Align2::LEFT_CENTER,
                ch,
                FontId::proportional(15.0),
                color,
            );
            if unread {
                p.circle_filled(rect.right_center() - egui::vec2(12.0, 0.0), 3.5, pal().text);
            }
            if resp.clicked() {
                act.select_text = Some(ch.clone());
            }
            if can_edit {
                channels_ui::row_admin(&resp, &mut self.channel_edit, false, ch, i);
            }
        }
        let end = ui.cursor().top();
        channels_ui::drop_target(ui, &mut self.channel_edit, false, &rows, end, act);
    }

    fn voice_list(
        &mut self,
        ui: &mut egui::Ui,
        speaking: &HashSet<u32>,
        can_edit: bool,
        act: &mut ListActions,
    ) {
        let Some(sess) = &self.session else { return };
        let sense = if can_edit {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let mut rows = Vec::new();
        for (i, ch) in sess.voice_channels.iter().enumerate() {
            let here = self.voice_channel.as_deref() == Some(ch.as_str());
            let members = sess.members(ch);
            if let Some(r) = channels_ui::rename_box(ui, &mut self.channel_edit, true, ch, act) {
                rows.push(r);
            } else {
                let (rect, resp) = Self::row_sense(ui, 30.0, false, sense);
                rows.push(rect);
                let p = ui.painter();
                p.text(
                    rect.left_center() + egui::vec2(10.0, 0.0),
                    Align2::LEFT_CENTER,
                    "🔊",
                    FontId::proportional(14.0),
                    if here { pal().accent } else { pal().faint },
                );
                p.text(
                    rect.left_center() + egui::vec2(32.0, 0.0),
                    Align2::LEFT_CENTER,
                    ch,
                    FontId::proportional(15.0),
                    if here { pal().text } else { pal().muted },
                );
                if members.len() >= sess.max_per_voice {
                    p.text(
                        rect.right_center() - egui::vec2(10.0, 0.0),
                        Align2::RIGHT_CENTER,
                        "Full",
                        FontId::proportional(12.0),
                        pal().faint,
                    );
                }
                if self.joining.as_deref() == Some(ch.as_str()) {
                    p.text(
                        rect.right_center() - egui::vec2(10.0, 0.0),
                        Align2::RIGHT_CENTER,
                        "Joining…",
                        FontId::proportional(12.0),
                        pal().accent,
                    );
                }
                if can_edit {
                    channels_ui::row_admin(&resp, &mut self.channel_edit, true, ch, i);
                }
                let resp = resp.on_hover_text(if here {
                    format!("You're in {ch}")
                } else {
                    format!("Join {ch}")
                });
                if resp.clicked() && !here {
                    act.join = Some(ch.clone());
                }
            }
            for m in members {
                let is_me = m.id == sess.me_id;
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let (rect, resp) = Self::row(ui, 30.0, false);
                    let talking = speaking.contains(&m.id);
                    let p = ui.painter();
                    paint_avatar(
                        p,
                        rect.left_center() + egui::vec2(18.0, 0.0),
                        11.0,
                        &m.name,
                        self.avatars.pic(ui.ctx(), &m.name),
                        talking,
                        pal().bg_deep,
                    );
                    let label = if is_me {
                        format!("{} (you)", m.name)
                    } else {
                        m.name.clone()
                    };
                    let locally_muted = !is_me && self.s.local_mutes.contains(&m.name);
                    let color = if talking {
                        pal().accent
                    } else if locally_muted {
                        pal().faint
                    } else {
                        pal().muted
                    };
                    p.text(
                        rect.left_center() + egui::vec2(38.0, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(14.5),
                        color,
                    );
                    // Status flags on the right: deafened shows muted too.
                    let x = members::paint_voice_flags(
                        p,
                        egui::pos2(rect.right() - 12.0, rect.center().y),
                        m.muted,
                        m.deafened,
                        12.0,
                    );
                    if locally_muted {
                        let c = egui::pos2(x, rect.center().y);
                        p.text(
                            c,
                            Align2::CENTER_CENTER,
                            "🔊",
                            FontId::proportional(12.0),
                            pal().red,
                        );
                        p.line_segment(
                            [c + egui::vec2(-6.0, -6.0), c + egui::vec2(6.0, 6.0)],
                            Stroke::new(1.5_f32, pal().red),
                        );
                    }
                    if !is_me {
                        let resp = resp.on_hover_text(format!("Volume for {}", m.name));
                        if resp.clicked() {
                            act.open_pop = Some(VolumePop {
                                name: m.name.clone(),
                                account: m.account,
                                pos: rect.right_top() + egui::vec2(8.0, 0.0),
                                opened_frame: ui.ctx().cumulative_frame_nr(),
                            });
                        }
                    }
                });
            }
        }
        let end = ui.cursor().top();
        channels_ui::drop_target(ui, &mut self.channel_edit, true, &rows, end, act);
    }

    pub(crate) fn select_text_channel(&mut self, ch: String) {
        if ch != self.current_text {
            self.player = None;
        }
        self.unread.remove(&ch);
        self.current_text = ch.clone();
        self.s.last_text_channel = Some(ch);
        self.focus_composer = true;
    }

    pub(crate) fn voice_panel(&mut self, ui: &mut egui::Ui) {
        let peers_connecting = self.joining.is_some();
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(
                dot.center(),
                4.5,
                if peers_connecting {
                    pal().accent
                } else {
                    pal().teal
                },
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let status = if peers_connecting {
                    "Joining…".to_string()
                } else {
                    match self.ping {
                        Some(ms) => format!("Voice connected · {ms} ms"),
                        None => "Voice connected".to_string(),
                    }
                };
                ui.label(
                    RichText::new(status)
                        .size(13.0)
                        .strong()
                        .color(if peers_connecting {
                            pal().accent
                        } else {
                            pal().teal
                        }),
                );
                let ch = self
                    .voice_channel
                    .clone()
                    .or(self.joining.clone())
                    .unwrap_or_default();
                ui.label(RichText::new(ch).size(13.0).color(pal().muted));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, "📞", false, pal().red, "Leave voice").clicked() {
                    self.leave_voice();
                }
            });
        });
    }

    pub(crate) fn me_row(&mut self, ui: &mut egui::Ui, speaking: &HashSet<u32>) {
        let Some(sess) = &self.session else { return };
        let me_name = sess.me_name.clone();
        let talking = speaking.contains(&sess.me_id);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::hover());
            paint_avatar(
                ui.painter(),
                rect.center(),
                15.0,
                &me_name,
                self.avatars.pic(ui.ctx(), &me_name),
                talking,
                pal().me_bg,
            );
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(egui::Label::new(RichText::new(&me_name).strong()).truncate());
                let hint = if self.s.deafened {
                    "Deafened".to_string()
                } else if self.s.muted {
                    "Muted".to_string()
                } else if self.s.push_to_talk {
                    format!("Push to talk: {}", keys::key_name(self.s.ptt_key))
                } else if self.voice_channel.is_some() {
                    "Mic on".to_string()
                } else {
                    "Online".to_string()
                };
                ui.add(
                    egui::Label::new(RichText::new(hint).size(12.0).color(pal().faint)).truncate(),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if bar_button(ui, "⚙", false, "Settings").clicked() {
                    self.settings_open = !self.settings_open;
                }
                let deaf_tip = if self.s.deafened {
                    "Undeafen"
                } else {
                    "Deafen"
                };
                if bar_button(ui, "🎧", self.s.deafened, deaf_tip).clicked() {
                    self.toggle_deafen();
                }
                let muted = self.s.muted || self.s.deafened;
                if bar_button(ui, "🎤", muted, if muted { "Unmute" } else { "Mute" }).clicked() {
                    self.toggle_mute();
                }
            });
        });
    }
}
