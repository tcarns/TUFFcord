//! The member list on the right, like Discord's: who's online (and in which
//! voice channel), then everyone else with an account. It folds away with the
//! people button at the top of the chat.

use crate::theme::{self, pal};
use crate::widgets::paint_avatar;
use crate::{chat_text, App, VolumePop};
use eframe::egui::{self, Align2, CornerRadius, FontId, Frame, Margin, Sense, Stroke};
use std::collections::{BTreeMap, HashSet};

/// Narrower windows hide the list (the button still shows it).
pub const MIN_WINDOW: f32 = 900.0;
const SLIDE_SECS: f32 = 0.15;

struct Row {
    name: String,
    account: Option<u32>,
    admin: bool,
    online: bool,
    me: bool,
    voice: Option<(String, bool, bool)>,
    talking: bool,
}

/// Draw the "muted" and "deafened" marks, right to left from `right`.
/// Deafened also shows muted (you can't talk while deafened).
pub fn paint_voice_flags(
    p: &egui::Painter,
    right: egui::Pos2,
    muted: bool,
    deafened: bool,
    size: f32,
) -> f32 {
    let mut x = right.x;
    let mut flag = |glyph: &str| {
        let c = egui::pos2(x, right.y);
        p.text(
            c,
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(size),
            pal().red,
        );
        let d = size * 0.5;
        p.line_segment(
            [c + egui::vec2(-d, -d), c + egui::vec2(d, d)],
            Stroke::new(1.5_f32, pal().red),
        );
        x -= size + 6.0;
    };
    if deafened {
        flag("🎧");
    }
    if muted || deafened {
        flag("🎤");
    }
    x
}

impl App {
    pub fn members_visible(&self, ctx: &egui::Context) -> bool {
        self.s.show_members && ctx.screen_rect().width() >= MIN_WINDOW && self.session.is_some()
    }

    /// The member list; returns its area once fully open (for the drag edge).
    pub fn member_panel(
        &mut self,
        ctx: &egui::Context,
        speaking: &HashSet<u32>,
    ) -> Option<egui::Rect> {
        // Slides open and shut; repaints only while it moves.
        let t = ctx.animate_bool_with_time_and_easing(
            egui::Id::new("members_open"),
            self.members_visible(ctx),
            SLIDE_SECS,
            egui::emath::easing::cubic_out,
        );
        if t <= 0.0 {
            return None;
        }
        let Some(sess) = &self.session else {
            return None;
        };

        // Who's in which voice channel, by session id.
        let mut in_voice: BTreeMap<u32, (String, bool, bool)> = BTreeMap::new();
        for ch in &sess.voice_state {
            for m in &ch.members {
                in_voice.insert(m.id, (ch.name.clone(), m.muted, m.deafened));
            }
        }
        let mut rows: Vec<Row> = Vec::new();
        let mut seen_accounts = HashSet::new();
        // Online first: one row per account (or per name for apps without accounts).
        let mut seen_names = HashSet::new();
        for u in &sess.users {
            let key = u
                .account
                .map(|a| a.to_string())
                .unwrap_or_else(|| u.name.clone());
            if !seen_names.insert(key) {
                continue;
            }
            if let Some(a) = u.account {
                seen_accounts.insert(a);
            }
            let ids: Vec<u32> = sess
                .users
                .iter()
                .filter(|x| {
                    (u.account.is_some() && x.account == u.account)
                        || (u.account.is_none() && x.name == u.name)
                })
                .map(|x| x.id)
                .collect();
            rows.push(Row {
                name: u.name.clone(),
                account: u.account,
                admin: u.admin
                    || sess
                        .members
                        .iter()
                        .any(|m| Some(m.account) == u.account && m.admin),
                online: true,
                me: ids.contains(&sess.me_id),
                voice: ids.iter().find_map(|id| in_voice.get(id).cloned()),
                talking: ids.iter().any(|id| speaking.contains(id)),
            });
        }
        for m in &sess.members {
            if seen_accounts.contains(&m.account) {
                continue;
            }
            rows.push(Row {
                name: m.name.clone(),
                account: Some(m.account),
                admin: m.admin,
                online: false,
                me: false,
                voice: None,
                talking: false,
            });
        }
        rows.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let online = rows.iter().filter(|r| r.online).count();
        let offline = rows.len() - online;
        let is_admin = self.is_admin();

        let mut open_pop: Option<VolumePop> = None;
        let bg = theme::mix(pal().bg, pal().bg_deep, 0.55);
        let (min, _, max) = tuffcord::settings::MEMBERS_WIDTH;
        let width = self.s.members_width.clamp(min, max);
        let panel = egui::SidePanel::right("members")
            .exact_width(width * t)
            .resizable(false)
            .frame(Frame::new().fill(bg).inner_margin(Margin {
                left: 10,
                right: 8,
                top: 14,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                // Lay the list out at full width while it slides; the panel clips the rest.
                let mut full = ui.max_rect();
                full.set_width(full.width() + width * (1.0 - t));
                ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            let header = |ui: &mut egui::Ui, text: String| {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(text)
                                        .size(12.0)
                                        .strong()
                                        .color(pal().faint),
                                );
                                ui.add_space(2.0);
                            };
                            header(ui, format!("ONLINE — {online}"));
                            for (i, r) in rows.iter().enumerate() {
                                if i == online && offline > 0 {
                                    ui.add_space(10.0);
                                    header(ui, format!("OFFLINE — {offline}"));
                                }
                                let clickable = (r.online && !r.me) || (!r.online && is_admin);
                                let h = if r.voice.is_some() { 44.0 } else { 36.0 };
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), h),
                                    if clickable {
                                        Sense::click()
                                    } else {
                                        Sense::hover()
                                    },
                                );
                                if clickable && resp.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(6),
                                        pal().raised,
                                    );
                                }
                                let pic = self.avatars.pic(ui.ctx(), &r.name);
                                let p = ui.painter();
                                let c = rect.left_center() + egui::vec2(20.0, 0.0);
                                paint_avatar(p, c, 14.0, &r.name, pic, r.talking, bg);
                                if !r.online {
                                    // Faded, like Discord's offline members.
                                    p.circle_filled(c, 15.0, theme::with_alpha(bg, 140));
                                } else {
                                    p.circle_filled(c + egui::vec2(10.0, 10.0), 5.5, bg);
                                    p.circle_filled(c + egui::vec2(10.0, 10.0), 4.0, pal().teal);
                                }
                                let name_color = if !r.online {
                                    pal().faint
                                } else if r.talking {
                                    pal().accent
                                } else {
                                    pal().text
                                };
                                let label = if r.me {
                                    format!("{} (you)", r.name)
                                } else {
                                    r.name.clone()
                                };
                                let name_y = if r.voice.is_some() {
                                    rect.center().y - 8.0
                                } else {
                                    rect.center().y
                                };
                                let g =
                                    p.layout_no_wrap(label, FontId::proportional(14.5), name_color);
                                let name_w = g.size().x.min(rect.width() - 80.0);
                                p.with_clip_rect(rect.shrink2(egui::vec2(0.0, 0.0)).intersect(
                                    egui::Rect::from_min_max(
                                        rect.min,
                                        egui::pos2(rect.right() - 26.0, rect.bottom()),
                                    ),
                                ))
                                .galley(
                                    egui::pos2(rect.left() + 42.0, name_y - g.size().y / 2.0),
                                    g,
                                    name_color,
                                );
                                if r.admin {
                                    let crown = egui::Rect::from_center_size(
                                        egui::pos2(rect.left() + 42.0 + name_w + 11.0, name_y),
                                        egui::vec2(15.0, 15.0),
                                    );
                                    if let Some(n) = tuffcord::twemoji::name_for("👑") {
                                        chat_text::paint(
                                            ui,
                                            &mut self.emoji.borrow_mut(),
                                            n,
                                            crown,
                                        );
                                    }
                                    let _ = ui
                                        .interact(crown, ui.id().with(("crown", i)), Sense::hover())
                                        .on_hover_text("Admin");
                                }
                                if let Some((ch, muted, deafened)) = &r.voice {
                                    let p = ui.painter();
                                    let y = rect.center().y + 10.0;
                                    p.text(
                                        egui::pos2(rect.left() + 42.0, y),
                                        Align2::LEFT_CENTER,
                                        format!("🔊 {ch}"),
                                        FontId::proportional(12.0),
                                        pal().muted,
                                    );
                                    paint_voice_flags(
                                        p,
                                        egui::pos2(rect.right() - 12.0, rect.center().y),
                                        *muted,
                                        *deafened,
                                        11.0,
                                    );
                                }
                                if clickable && resp.clicked() {
                                    open_pop = Some(VolumePop {
                                        name: r.name.clone(),
                                        account: r.account,
                                        pos: rect.left_top() - egui::vec2(8.0, 0.0),
                                        opened_frame: ui.ctx().cumulative_frame_nr(),
                                    });
                                }
                            }
                            ui.add_space(8.0);
                        });
                });
            })
            .response
            .rect;
        if let Some(mut p) = open_pop {
            // Open to the left of the list.
            p.pos.x -= 250.0;
            self.pop = Some(p);
        }
        (t >= 1.0).then_some(panel)
    }

    /// The people button in the chat header.
    pub fn members_button(&mut self, ui: &mut egui::Ui) {
        let shown = self.s.show_members;
        let narrow = ui.ctx().screen_rect().width() < MIN_WINDOW;
        let tip = if narrow {
            "Make the window wider to see the member list"
        } else if shown {
            "Hide the member list"
        } else {
            "Show the member list"
        };
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 28.0), Sense::click());
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(6), pal().raised);
        }
        let color = if shown && !narrow {
            pal().text
        } else {
            pal().faint
        };
        // Two little people.
        let c = rect.center();
        for (dx, r, alpha) in [(-4.0, 1.0, 255u8), (5.0, 0.85, 170u8)] {
            let col = theme::with_alpha(color, alpha);
            p.circle_filled(c + egui::vec2(dx, -4.0), 3.6 * r, col);
            let body = egui::Rect::from_center_size(
                c + egui::vec2(dx, 5.5),
                egui::vec2(10.0 * r, 7.0 * r),
            );
            p.rect_filled(
                body,
                CornerRadius {
                    nw: 5,
                    ne: 5,
                    sw: 1,
                    se: 1,
                },
                col,
            );
        }
        if resp.on_hover_text(tip).clicked() && !narrow {
            self.s.show_members = !shown;
            self.s.save();
        }
    }
}
