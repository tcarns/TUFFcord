//! Settings > Account, the security parts: your signed-in devices (sign one
//! out, or all the others), and for admins the admin log. (Invites are made in
//! the server console.)
//! Each needs a server that says it supports it (0.11+).

use crate::accounts::when;
use crate::theme::pal;
use crate::App;
use eframe::egui::{self, RichText};
use proto::{AdminLogEntry, ClientMsg, ServerMsg, SessionInfo};

#[derive(Default)]
pub(crate) struct Security {
    sessions: bool,
    admin_log: bool,
    devices: Option<Vec<SessionInfo>>,
    asked_devices: bool,
    log: Option<Vec<AdminLogEntry>>,
}

impl Security {
    pub fn new(features: &[String]) -> Security {
        let has = |f: &str| features.iter().any(|x| x == f);
        Security {
            sessions: has(proto::FEATURE_SESSIONS),
            admin_log: has(proto::FEATURE_ADMIN_LOG),
            ..Default::default()
        }
    }
}

fn heading(ui: &mut egui::Ui, t: &str) {
    ui.label(RichText::new(t).size(13.0).strong().color(pal().muted));
}

fn faint(ui: &mut egui::Ui, t: &str) {
    ui.add(egui::Label::new(RichText::new(t).size(12.5).color(pal().faint)).wrap());
}

impl App {
    /// Sessions and AdminLog answers. True if `msg` was one.
    pub(crate) fn on_security_msg(&mut self, msg: ServerMsg) -> bool {
        let Some(sec) = self.session.as_mut().map(|s| &mut s.security) else {
            return false;
        };
        match msg {
            ServerMsg::Sessions { list } => sec.devices = Some(list),
            ServerMsg::AdminLog { entries } => sec.log = Some(entries),
            _ => return false,
        }
        true
    }

    /// Below the account settings.
    pub(crate) fn security_settings(&mut self, ui: &mut egui::Ui) {
        let admin = self.acct.me.as_ref().is_some_and(|m| m.admin);
        let Some(sec) = self.session.as_mut().map(|s| &mut s.security) else {
            return;
        };
        let mut send = Vec::new();
        if sec.sessions {
            if !sec.asked_devices {
                sec.asked_devices = true;
                send.push(ClientMsg::Sessions);
            }
            heading(ui, "Signed-in devices");
            faint(
                ui,
                "Everywhere your account is remembered. Sign out anything you don't recognize, then change your password.",
            );
            ui.add_space(4.0);
            match &sec.devices {
                None => faint(ui, "Loading…"),
                Some(list) => {
                    for d in list {
                        ui.horizontal(|ui| {
                            let place = if d.ip.is_empty() {
                                "unknown address"
                            } else {
                                &d.ip
                            };
                            let text = format!("{place} · used {}", when(d.last_used));
                            ui.label(RichText::new(text).size(12.5).color(pal().text));
                            if d.current {
                                ui.label(
                                    RichText::new("this device").size(12.0).color(pal().accent),
                                );
                            } else if ui.small_button("Sign out").clicked() {
                                send.push(ClientMsg::EndSession {
                                    id: Some(d.id.clone()),
                                });
                            }
                        });
                    }
                    let others = list.iter().any(|d| !d.current);
                    if others && ui.button("Sign out everywhere else").clicked() {
                        send.push(ClientMsg::EndSession { id: None });
                    }
                }
            }
            ui.add_space(14.0);
        }
        if admin && sec.admin_log {
            heading(ui, "Admin log");
            if sec.log.is_none() {
                if ui.button("Show what admins did").clicked() {
                    send.push(ClientMsg::AdminLog);
                }
            } else if ui.small_button("Refresh").clicked() {
                send.push(ClientMsg::AdminLog);
            }
            if let Some(entries) = &sec.log {
                if entries.is_empty() {
                    faint(ui, "Nothing yet.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("admin_log")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for e in entries {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(when(e.ts)).size(12.0).color(pal().faint));
                                ui.label(RichText::new(&e.text).size(12.5).color(pal().text));
                            });
                        }
                    });
            }
            ui.add_space(14.0);
        }
        for m in send {
            self.net.send(m);
        }
    }
}
