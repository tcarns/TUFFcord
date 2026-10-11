//! Accounts in the app: signing in or creating an account, your account in
//! Settings, and the admin tools (people list, kick, ban, reset password).

use crate::theme::pal;
use crate::widgets::primary_button;
use crate::{App, Conn};
use eframe::egui::{
    self, Align, CornerRadius, FontId, Frame, Key, Layout, Margin, RichText, Sense, Stroke,
};
use proto::{Account, AccountSummary, AdminAction, ClientMsg, ServerMsg};
use std::collections::HashMap;
use tuffcord::net::{self, SignIn};

#[derive(PartialEq, Clone, Copy, Default)]
pub enum LoginMode {
    #[default]
    SignIn,
    Create,
}

#[derive(Default)]
pub struct AccountUi {
    pub mode: LoginMode,
    /// Typed on the sign-in screen; never saved.
    pub password: String,
    pub confirm: String,
    /// The group password, for creating an account.
    pub invite: String,
    /// A hint (not an error) on the sign-in screen.
    pub notice: Option<String>,
    /// Who you are, on a server with accounts.
    pub me: Option<Account>,
    /// The sign-in in use now (handed to the new copy when the app updates).
    pub token: Option<String>,
    /// The group password in use, on a server without accounts.
    pub group_password: Option<String>,
    /// How the last connection was started.
    pub last_sign_in: Option<SignIn>,
    /// Signed in with a temporary password: must pick a new one.
    pub must_change: bool,
    /// Admins: everyone's accounts.
    pub list: Vec<AccountSummary>,
    /// Admins: what the last action did, and a temporary password to pass on.
    pub result: Option<(String, Option<String>)>,
    /// Admins: an action waiting for "are you sure?".
    pub confirm_action: Option<(String, String, AdminAction)>,
    // Settings → Account
    pub new_name: String,
    pub old_pw: String,
    pub new_pw: String,
    pub new_pw2: String,
    pub form_msg: Option<(String, bool)>,
    /// Account id → the name last seen, to carry volume settings over renames.
    pub known_names: HashMap<u32, String>,
}

fn field(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    value: &mut String,
    secret: bool,
    limit: usize,
    enabled: bool,
) -> bool {
    ui.label(RichText::new(label).size(13.0).color(pal().muted).strong());
    let edit = egui::TextEdit::singleline(value)
        .hint_text(RichText::new(hint).color(pal().faint))
        .password(secret)
        .desired_width(f32::INFINITY)
        .margin(Margin::symmetric(10, 8))
        .char_limit(limit);
    let r = ui.add_enabled(enabled, edit);
    ui.add_space(6.0);
    r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// Settings and dialogs size themselves to their content, so fields there get a
/// fixed width (a field that takes "all the room there is" would make them grow).
const FORM_W: f32 = 400.0;
const DIALOG_W: f32 = 340.0;

fn small_field(
    ui: &mut egui::Ui,
    hint: &str,
    value: &mut String,
    secret: bool,
    width: f32,
) -> egui::Response {
    // A forced size: in a window that fits its content, a text box sized from
    // "the room available" (or from its hint) makes the window grow every frame.
    ui.add_sized(
        [width, 28.0],
        egui::TextEdit::singleline(value)
            .hint_text(RichText::new(hint).color(pal().faint))
            .password(secret)
            .margin(Margin::symmetric(8, 6))
            .char_limit(128),
    )
}

fn dialog_frame(ctx: &egui::Context) -> Frame {
    Frame::popup(&ctx.style())
        .fill(pal().bg)
        .stroke(Stroke::new(1.0_f32, pal().line))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(22))
}

fn tab(ui: &mut egui::Ui, text: &str, selected: bool, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 34.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(8), pal().raised_2);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), pal().raised);
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.5),
        if selected { pal().text } else { pal().muted },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn when(ms: u64) -> String {
    if ms == 0 {
        return "never".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mins = now.saturating_sub(ms) / 60_000;
    match mins {
        0 => "just now".into(),
        1..=59 => format!("{mins} min ago"),
        60..=1439 => format!("{} h ago", mins / 60),
        _ => format!("{} days ago", mins / 1440),
    }
}

impl App {
    // ------------------------------------------------------------ connecting

    /// Connect to the server in settings, signing in with `sign_in`.
    pub fn sign_in_with(&mut self, sign_in: SignIn) {
        let url = match net::normalize_server(&self.s.server) {
            Ok(u) => u,
            Err(e) => {
                self.login_error = Some(e);
                return;
            }
        };
        let name = self.s.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() && !matches!(sign_in, SignIn::Token { .. }) {
            self.login_error = Some("Enter your name.".into());
            return;
        }
        self.login_error = None;
        self.acct.notice = None;
        self.conn = Conn::Connecting;
        self.acct.last_sign_in = Some(sign_in.clone());
        self.net.connect(url, name, sign_in);
    }

    /// Sign in with what's saved, if anything. True if connecting.
    pub fn connect_saved(&mut self, token: String, group_password: String) -> bool {
        if self.s.server.is_empty() {
            return false;
        }
        if !token.is_empty() {
            self.sign_in_with(SignIn::Token { token });
        } else if !group_password.is_empty() && !self.s.name.is_empty() {
            // Saved by an older version (or for a server without accounts). A server
            // with accounts will answer "account_required", which opens "Create account".
            self.sign_in_with(SignIn::Group {
                password: group_password,
            });
        } else {
            return false;
        }
        self.conn != Conn::Offline
    }

    fn submit_login(&mut self) {
        let password = self.acct.password.clone();
        match self.acct.mode {
            LoginMode::SignIn => {
                if password.is_empty() {
                    // Still signed in on this computer (e.g. after being kicked)?
                    if !self.s.token.is_empty() {
                        let token = self.s.token.clone();
                        self.sign_in_with(SignIn::Token { token });
                    } else {
                        self.login_error = Some("Enter your password.".into());
                    }
                    return;
                }
                self.sign_in_with(SignIn::Login { password });
            }
            LoginMode::Create => {
                if self.s.name.trim().chars().count() < 2 {
                    self.login_error = Some("Names need at least 2 characters.".into());
                } else if password.chars().count() < 8 {
                    self.login_error = Some("Passwords need at least 8 characters.".into());
                } else if password != self.acct.confirm {
                    self.login_error = Some("The two passwords don't match.".into());
                } else {
                    self.sign_in_with(SignIn::Register {
                        group_password: self.acct.invite.clone(),
                        new_password: password,
                    });
                }
            }
        }
    }

    /// The server stopped us signing in (or threw us out).
    pub fn on_failed(&mut self, message: String, code: Option<String>) {
        tuffcord::applog::warn(format!(
            "Couldn't sign in to {}: {message} ({})",
            self.s.server,
            code.as_deref().unwrap_or("no code")
        ));
        self.leave_voice_quietly();
        self.session = None;
        self.conn = Conn::Offline;
        self.banner = None;
        self.acct.me = None;
        self.acct.token = None;
        self.acct.list.clear();
        self.acct.must_change = false;
        match code.as_deref() {
            Some("account_required") => {
                // Signed in the old way to a server that now has accounts:
                // the group password they used is what "Create account" needs.
                if let Some(SignIn::Group { password }) = &self.acct.last_sign_in {
                    self.acct.invite = password.clone();
                } else if self.acct.invite.is_empty() {
                    self.acct.invite = self.s.password.clone();
                }
                self.acct.mode = LoginMode::Create;
                self.acct.notice = Some(message);
                self.login_error = None;
                self.s.auto_connect = false;
                self.s.save();
                return;
            }
            Some("session_expired") | Some("banned") | Some("password_reset") => {
                self.s.token.clear();
                self.s.auto_connect = false;
                self.acct.mode = LoginMode::SignIn;
                self.s.save();
            }
            Some("kicked") => {
                self.s.auto_connect = false;
                self.s.save();
            }
            _ => {}
        }
        self.login_error = Some(message);
    }

    /// Account parts of the server's welcome.
    pub fn on_welcome(
        &mut self,
        account: Option<Account>,
        token: Option<String>,
        must_change: bool,
    ) {
        self.acct.password.clear();
        self.acct.confirm.clear();
        self.acct.notice = None;
        self.acct.must_change = must_change;
        if let Some(a) = account {
            self.s.name = a.name.clone();
            self.acct.new_name = a.name.clone();
            self.acct.me = Some(a);
            self.acct.group_password = None;
            if let Some(t) = token {
                if self.s.remember {
                    self.s.token = t.clone();
                }
                self.acct.token = Some(t);
            }
            // Accounts replace the group password; don't keep it around.
            self.s.password.clear();
            self.acct.invite.clear();
        } else {
            // A server from before accounts: signed in the old way.
            self.acct.me = None;
            self.acct.token = None;
            let group = match &self.acct.last_sign_in {
                Some(SignIn::Group { password }) => Some(password.clone()),
                Some(SignIn::Register { group_password, .. }) => Some(group_password.clone()),
                _ => None,
            };
            if let Some(g) = group {
                if self.s.remember {
                    self.s.password = g.clone();
                }
                self.acct.group_password = Some(g);
            }
            if matches!(self.acct.last_sign_in, Some(SignIn::Register { .. })) {
                self.show_banner(
                    "This server hasn't been updated for accounts yet, so you joined with the group password.",
                    false,
                    Some(10),
                );
            }
        }
    }

    /// Account messages from the server. True if `msg` was one.
    pub fn on_account_msg(&mut self, msg: &ServerMsg) -> bool {
        match msg {
            ServerMsg::AccountUpdated { account, token } => {
                self.s.name = account.name.clone();
                self.acct.new_name = account.name.clone();
                if let Some(sess) = self.session.as_mut() {
                    sess.me_name = account.name.clone();
                }
                if let Some(t) = token {
                    // A new password: the old sign-ins were replaced.
                    if self.s.remember {
                        self.s.token = t.clone();
                    }
                    self.acct.token = Some(t.clone());
                    self.acct.must_change = false;
                    self.acct.old_pw.clear();
                    self.acct.new_pw.clear();
                    self.acct.new_pw2.clear();
                }
                if !account.admin {
                    self.acct.list.clear();
                }
                self.acct.me = Some(account.clone());
                self.s.save();
            }
            ServerMsg::Accounts { list } => self.acct.list = list.clone(),
            ServerMsg::AdminResult { text, secret } => {
                self.acct.result = Some((text.clone(), secret.clone()));
            }
            ServerMsg::Notice { text } => {
                self.acct.form_msg = Some((text.clone(), false));
                self.show_banner(text.clone(), false, Some(6));
            }
            _ => return false,
        }
        true
    }

    /// People keep their volume and "mute for me" when they rename.
    pub fn follow_renames(&mut self) {
        let Some(sess) = &self.session else { return };
        let mut moved = false;
        for u in &sess.users {
            let Some(id) = u.account else { continue };
            match self.acct.known_names.insert(id, u.name.clone()) {
                Some(old) if old != u.name => {
                    if let Some(v) = self.s.volumes.remove(&old) {
                        self.s.volumes.insert(u.name.clone(), v);
                        moved = true;
                    }
                    if self.s.local_mutes.remove(&old) {
                        self.s.local_mutes.insert(u.name.clone());
                        moved = true;
                    }
                }
                _ => {}
            }
        }
        if moved {
            self.s.save();
            self.apply_gains();
        }
    }

    /// Before disconnecting on purpose: forget this device's sign-in on the server.
    pub fn forget_sign_in(&mut self) {
        if self.acct.me.is_some() {
            self.net.send(ClientMsg::SignOut);
        }
        self.s.token.clear();
        self.acct.me = None;
        self.acct.token = None;
        self.acct.list.clear();
        self.acct.must_change = false;
    }

    pub fn is_admin(&self) -> bool {
        self.acct.me.as_ref().is_some_and(|a| a.admin)
    }

    // ------------------------------------------------------------ sign-in screen

    pub fn login_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(pal().bg_deep))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let card_h = if self.acct.mode == LoginMode::Create {
                            620.0
                        } else {
                            470.0
                        };
                        ui.add_space(((ui.available_height() - card_h) / 2.0).max(12.0));
                        ui.vertical_centered(|ui| {
                            Frame::new()
                                .fill(pal().bg)
                                .stroke(Stroke::new(1.0_f32, pal().line))
                                .corner_radius(CornerRadius::same(16))
                                .inner_margin(Margin::same(28))
                                .show(ui, |ui| {
                                    ui.set_width(330.0);
                                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                        self.login_card(ui);
                                    });
                                });
                            let latest = self.update.lock().latest.clone();
                            if let Some(r) = latest {
                                ui.add_space(14.0);
                                ui.horizontal(|ui| {
                                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                                    ui.label(
                                        RichText::new(format!(
                                            "TUFFcord {} is available.",
                                            r.version
                                        ))
                                        .color(pal().muted),
                                    );
                                    ui.hyperlink_to(
                                        RichText::new("Download it").color(pal().accent),
                                        &r.url,
                                    );
                                });
                            }
                            ui.add_space(16.0);
                        });
                    });
            });
    }

    fn login_card(&mut self, ui: &mut egui::Ui) {
        // Logo (three level bars) beside the name.
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 36.0), Sense::hover());
            for (i, h) in [16.0f32, 34.0, 24.0].iter().enumerate() {
                let x = rect.left() + i as f32 * 13.0;
                let r = egui::Rect::from_min_max(
                    egui::pos2(x, rect.center().y - h / 2.0),
                    egui::pos2(x + 8.0, rect.center().y + h / 2.0),
                );
                ui.painter()
                    .rect_filled(r, CornerRadius::same(4), pal().accent);
            }
            ui.add_space(6.0);
            ui.label(RichText::new("TUFFcord").size(26.0).strong());
        });
        ui.label(RichText::new("Voice and chat for the group.").color(pal().muted));
        ui.add_space(12.0);

        let busy = self.conn != Conn::Offline;
        // Sign in | Create account
        Frame::new()
            .fill(pal().bg_deep)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    let w = (ui.available_width() / 2.0).floor();
                    for (mode, text) in [
                        (LoginMode::SignIn, "Sign in"),
                        (LoginMode::Create, "Create account"),
                    ] {
                        if tab(ui, text, self.acct.mode == mode, w).clicked()
                            && !busy
                            && self.acct.mode != mode
                        {
                            self.acct.mode = mode;
                            self.login_error = None;
                        }
                    }
                });
            });
        ui.add_space(12.0);

        let mut submit = false;
        submit |= field(
            ui,
            "Server address",
            "abc.trycloudflare.com",
            &mut self.s.server,
            false,
            300,
            !busy,
        );
        match self.acct.mode {
            LoginMode::SignIn => {
                submit |= field(ui, "Your name", "", &mut self.s.name, false, 32, !busy);
                submit |= field(
                    ui,
                    "Password",
                    "",
                    &mut self.acct.password,
                    true,
                    128,
                    !busy,
                );
            }
            LoginMode::Create => {
                submit |= field(
                    ui,
                    "Pick a name",
                    "What friends will see",
                    &mut self.s.name,
                    false,
                    32,
                    !busy,
                );
                submit |= field(
                    ui,
                    "Pick a password",
                    "At least 6 characters",
                    &mut self.acct.password,
                    true,
                    128,
                    !busy,
                );
                submit |= field(
                    ui,
                    "Type it again",
                    "",
                    &mut self.acct.confirm,
                    true,
                    128,
                    !busy,
                );
                submit |= field(
                    ui,
                    "Group password",
                    "Ask whoever runs the server",
                    &mut self.acct.invite,
                    true,
                    300,
                    !busy,
                );
            }
        }
        ui.checkbox(&mut self.s.remember, "Keep me signed in on this computer");
        ui.add_space(4.0);
        if let Some(n) = &self.acct.notice {
            ui.add(egui::Label::new(RichText::new(n).color(pal().teal).size(13.0)).wrap());
            ui.add_space(4.0);
        }
        if let Some(err) = &self.login_error {
            ui.add(egui::Label::new(RichText::new(err).color(pal().red).size(13.0)).wrap());
            ui.add_space(4.0);
        }
        ui.add_space(6.0);
        let label = match (busy, self.acct.mode) {
            (true, _) => "Connecting…",
            (false, LoginMode::SignIn) => "Sign in",
            (false, LoginMode::Create) => "Create account",
        };
        if primary_button(ui, label, !busy).clicked() || (submit && !busy) {
            self.submit_login();
        }
        if busy {
            ui.add_space(6.0);
            if ui.small_button("Cancel").clicked() {
                self.net.disconnect();
                self.conn = Conn::Offline;
            }
        } else if self.acct.mode == LoginMode::SignIn {
            ui.add_space(8.0);
            ui.label(
                RichText::new("New here? Choose \"Create account\" and use the group password.")
                    .size(12.5)
                    .color(pal().faint),
            );
        }
    }

    // ------------------------------------------------------------ settings

    /// The Account part of Settings (and People, for admins).
    pub fn account_settings(&mut self, ui: &mut egui::Ui) {
        let heading = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(t).size(13.0).strong().color(pal().muted));
        };
        heading(ui, "Account");
        let Some(me) = self.acct.me.clone() else {
            ui.label(
                RichText::new(
                    "This server doesn't have accounts yet. You joined with the group password.",
                )
                .size(12.5)
                .color(pal().faint),
            );
            ui.add_space(14.0);
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Signed in as {}", me.name)).color(pal().text));
            if me.admin {
                ui.label(RichText::new("admin").size(12.0).color(pal().accent));
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Name").size(12.5).color(pal().faint));
        ui.horizontal(|ui| {
            let changed =
                self.acct.new_name.trim() != me.name && !self.acct.new_name.trim().is_empty();
            ui.add_sized(
                [FORM_W - 90.0, 28.0],
                egui::TextEdit::singleline(&mut self.acct.new_name)
                    .margin(Margin::symmetric(8, 6))
                    .char_limit(32),
            );
            if ui
                .add_enabled(changed, egui::Button::new("Rename"))
                .clicked()
            {
                self.acct.form_msg = None;
                self.net.send(ClientMsg::Rename {
                    name: self.acct.new_name.trim().to_string(),
                });
            }
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new("Change password")
                .size(12.5)
                .color(pal().faint),
        );
        small_field(ui, "Current password", &mut self.acct.old_pw, true, FORM_W);
        small_field(
            ui,
            "New password (at least 8 characters)",
            &mut self.acct.new_pw,
            true,
            FORM_W,
        );
        small_field(
            ui,
            "New password again",
            &mut self.acct.new_pw2,
            true,
            FORM_W,
        );
        ui.add_space(2.0);
        let ready = !self.acct.old_pw.is_empty() && !self.acct.new_pw.is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("Change password"))
            .clicked()
        {
            if let Some(e) = self.check_new_password() {
                self.acct.form_msg = Some((e, true));
            } else {
                self.acct.form_msg = None;
                self.net.send(ClientMsg::ChangePassword {
                    old_password: self.acct.old_pw.clone(),
                    new_password: self.acct.new_pw.clone(),
                });
            }
        }
        if let Some((m, err)) = &self.acct.form_msg {
            ui.add(
                egui::Label::new(RichText::new(m).size(12.5).color(if *err {
                    pal().red
                } else {
                    pal().teal
                }))
                .wrap(),
            );
        }
        ui.add_space(14.0);

        if me.admin {
            heading(ui, "People");
            ui.label(
                RichText::new("Everyone with an account. Use … to kick, ban, reset a password or make an admin.")
                    .size(12.5)
                    .color(pal().faint),
            );
            ui.add_space(4.0);
            let list = self.acct.list.clone();
            for a in &list {
                ui.horizontal(|ui| {
                    let dot = if a.online { pal().teal } else { pal().faint };
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, dot);
                    let cell = |ui: &mut egui::Ui, w: f32, text: RichText| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(w, 20.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.set_width(w);
                                ui.add(egui::Label::new(text).truncate());
                            },
                        );
                    };
                    cell(
                        ui,
                        130.0,
                        RichText::new(&a.name).color(if a.banned {
                            pal().faint
                        } else {
                            pal().text
                        }),
                    );
                    let mut tags = Vec::new();
                    if a.admin {
                        tags.push("admin".to_string());
                    }
                    if a.banned {
                        tags.push("banned".to_string());
                    }
                    if !a.online {
                        tags.push(format!("seen {}", when(a.last_seen)));
                    }
                    cell(
                        ui,
                        175.0,
                        RichText::new(tags.join(" · "))
                            .size(12.0)
                            .color(pal().faint),
                    );
                    if a.id != me.id {
                        ui.menu_button(" … ", |ui| {
                            if let Some(action) = self.admin_menu(ui, a) {
                                self.queue_admin(action, a.name.clone());
                                ui.close();
                            }
                        });
                    }
                });
            }
            ui.add_space(14.0);
        }
    }

    fn check_new_password(&self) -> Option<String> {
        if self.acct.new_pw.chars().count() < 8 {
            Some("Passwords need at least 8 characters.".into())
        } else if self.acct.new_pw != self.acct.new_pw2 {
            Some("The two new passwords don't match.".into())
        } else {
            None
        }
    }

    // ------------------------------------------------------------ admin actions

    /// Menu items for one account. Returns the one picked.
    fn admin_menu(&self, ui: &mut egui::Ui, a: &AccountSummary) -> Option<AdminAction> {
        let id = a.id;
        let mut picked = None;
        if a.online && ui.button("Kick").clicked() {
            picked = Some(AdminAction::Kick { account: id });
        }
        if ui.button("Reset password…").clicked() {
            picked = Some(AdminAction::ResetPassword { account: id });
        }
        if a.banned {
            if ui.button("Unban").clicked() {
                picked = Some(AdminAction::Unban { account: id });
            }
        } else if !a.admin && ui.button(RichText::new("Ban").color(pal().red)).clicked() {
            picked = Some(AdminAction::Ban { account: id });
        }
        if a.admin {
            if ui.button("Remove admin").clicked() {
                picked = Some(AdminAction::SetAdmin {
                    account: id,
                    admin: false,
                });
            }
        } else if !a.banned && ui.button("Make admin").clicked() {
            picked = Some(AdminAction::SetAdmin {
                account: id,
                admin: true,
            });
        }
        picked
    }

    /// Admin buttons in the popup you get by clicking someone. True if one was used.
    pub fn admin_popup_buttons(&mut self, ui: &mut egui::Ui, account: u32, name: &str) -> bool {
        if !self.is_admin() || self.acct.me.as_ref().is_some_and(|m| m.id == account) {
            return false;
        }
        let summary = self
            .acct
            .list
            .iter()
            .find(|a| a.id == account)
            .cloned()
            .unwrap_or(AccountSummary {
                id: account,
                name: name.to_string(),
                admin: false,
                banned: false,
                online: true,
                last_seen: 0,
            });
        ui.add_space(6.0);
        ui.separator();
        ui.label(RichText::new("Admin").size(12.5).color(pal().faint));
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Kick").clicked() {
                picked = Some(AdminAction::Kick { account });
            }
            if ui.button("Reset password…").clicked() {
                picked = Some(AdminAction::ResetPassword { account });
            }
            if !summary.admin && ui.button(RichText::new("Ban").color(pal().red)).clicked() {
                picked = Some(AdminAction::Ban { account });
            }
        });
        match picked {
            Some(action) => {
                self.queue_admin(action, name.to_string());
                true
            }
            None => false,
        }
    }

    /// Ask "are you sure?" for actions that matter; do the rest straight away.
    fn queue_admin(&mut self, action: AdminAction, name: String) {
        let (question, button) = match &action {
            AdminAction::Kick { .. } => (format!("Kick {name}? They'll be disconnected, but can sign back in."), "Kick"),
            AdminAction::Ban { .. } => (
                format!("Ban {name}? They'll be disconnected and can't sign back in or make a new account from the same address."),
                "Ban",
            ),
            AdminAction::ResetPassword { .. } => (
                format!("Reset {name}'s password? They'll be signed out everywhere, and you'll get a temporary password to send them."),
                "Reset password",
            ),
            AdminAction::SetAdmin { admin: true, .. } => (
                format!("Make {name} an admin? They'll be able to kick, ban and reset passwords."),
                "Make admin",
            ),
            _ => {
                self.net.send(ClientMsg::Admin { action });
                return;
            }
        };
        self.acct.confirm_action = Some((question, button.to_string(), action));
    }

    // ------------------------------------------------------------ dialogs

    pub fn account_dialogs(&mut self, ctx: &egui::Context) {
        if self.acct.must_change {
            self.must_change_dialog(ctx);
        }
        if let Some((question, button, action)) = self.acct.confirm_action.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::new("admin_confirm"))
                .frame(dialog_frame(ctx))
                .show(ctx, |ui| {
                    ui.set_width(DIALOG_W);
                    ui.add(egui::Label::new(RichText::new(&question).size(14.5)).wrap());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape))
                        {
                            close = true;
                        }
                        let b = egui::Button::new(
                            RichText::new(&button).color(pal().accent_ink).strong(),
                        )
                        .fill(pal().accent);
                        if ui.add(b).clicked() {
                            self.net.send(ClientMsg::Admin {
                                action: action.clone(),
                            });
                            close = true;
                        }
                    });
                });
            if close {
                self.acct.confirm_action = None;
            }
        }
        if let Some((text, secret)) = self.acct.result.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::new("admin_result"))
                .frame(dialog_frame(ctx))
                .show(ctx, |ui| {
                    ui.set_width(DIALOG_W);
                    ui.add(egui::Label::new(RichText::new(&text).size(14.5)).wrap());
                    if let Some(s) = &secret {
                        ui.add_space(10.0);
                        Frame::new()
                            .fill(pal().bg_deep)
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(s).monospace().size(18.0).color(pal().text),
                                    );
                                    if ui.button("Copy").clicked() {
                                        ui.ctx().copy_text(s.clone());
                                    }
                                });
                            });
                    }
                    ui.add_space(12.0);
                    if ui.button("Done").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                });
            if close {
                self.acct.result = None;
            }
        }
    }

    fn must_change_dialog(&mut self, ctx: &egui::Context) {
        let mut save = false;
        let mut sign_out = false;
        egui::Modal::new(egui::Id::new("must_change")).frame(dialog_frame(ctx)).show(ctx, |ui| {
            ui.set_width(DIALOG_W);
            ui.label(RichText::new("Pick a new password").size(18.0).strong());
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new("You signed in with a temporary password. Choose your own to keep using TUFFcord.")
                        .color(pal().muted),
                )
                .wrap(),
            );
            ui.add_space(10.0);
            small_field(ui, "New password (at least 8 characters)", &mut self.acct.new_pw, true, DIALOG_W);
            let r = small_field(ui, "New password again", &mut self.acct.new_pw2, true, DIALOG_W);
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                save = true;
            }
            if let Some((m, err)) = &self.acct.form_msg {
                ui.add(
                    egui::Label::new(RichText::new(m).size(12.5).color(if *err { pal().red } else { pal().teal }))
                        .wrap(),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Save").color(pal().accent_ink).strong()).fill(pal().accent);
                if ui.add(b).clicked() {
                    save = true;
                }
                if ui.button("Sign out").clicked() {
                    sign_out = true;
                }
            });
        });
        if save {
            if let Some(e) = self.check_new_password() {
                self.acct.form_msg = Some((e, true));
            } else {
                self.acct.form_msg = None;
                self.net.send(ClientMsg::ChangePassword {
                    old_password: String::new(),
                    new_password: self.acct.new_pw.clone(),
                });
            }
        }
        if sign_out {
            self.sign_out();
        }
    }
}
