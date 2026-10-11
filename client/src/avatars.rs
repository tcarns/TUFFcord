//! Profile pictures in the app: which member has which picture, fetching and
//! decoding only the ones drawn on screen, and choosing your own in
//! Settings > Account.

use crate::theme::pal;
use crate::widgets::paint_avatar;
use crate::App;
use eframe::egui::{self, ColorImage, RichText, Sense, TextureHandle, TextureId};
use proto::Member;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use tuffcord::avatar;
use tuffcord::files::Endpoint;

/// A picture once fetched and decoded (None while loading, or if it failed).
type Loaded = (String, Option<ColorImage>);

pub(crate) struct Avatars {
    endpoint: Option<Endpoint>,
    /// Member name -> (account, picture hash).
    by_name: HashMap<String, (u32, String)>,
    /// Picture hash -> texture; `None` while loading or after a failure.
    textures: RefCell<HashMap<String, Option<TextureHandle>>>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    /// Choosing or sending your picture: (still busy, what happened).
    status: Option<(bool, String)>,
    done: Option<Receiver<Result<&'static str, String>>>,
}

impl Default for Avatars {
    fn default() -> Self {
        let (tx, rx) = channel();
        Avatars {
            endpoint: None,
            by_name: HashMap::new(),
            textures: RefCell::new(HashMap::new()),
            tx,
            rx,
            status: None,
            done: None,
        }
    }
}

impl Avatars {
    /// Signed in (again): where to fetch from, and everyone's pictures.
    pub(crate) fn reset(&mut self, endpoint: Option<Endpoint>, members: &[Member]) {
        if self.endpoint.as_ref().map(|e| &e.base) != endpoint.as_ref().map(|e| &e.base) {
            self.textures.get_mut().clear();
        }
        self.endpoint = endpoint;
        self.set_members(members);
    }

    /// The member list changed. Pictures nobody uses any more are dropped.
    pub(crate) fn set_members(&mut self, members: &[Member]) {
        self.by_name = members
            .iter()
            .filter_map(|m| Some((m.name.clone(), (m.account, m.avatar.clone()?))))
            .collect();
        let used: std::collections::HashSet<&String> =
            self.by_name.values().map(|(_, h)| h).collect();
        self.textures.get_mut().retain(|h, _| used.contains(h));
    }

    /// `name`'s picture, if they have one and it's ready. The first call for a
    /// picture starts fetching it (from the disk cache when it's there).
    pub(crate) fn pic(&self, ctx: &egui::Context, name: &str) -> Option<TextureId> {
        let (account, hash) = self.by_name.get(name)?;
        let mut textures = self.textures.borrow_mut();
        while let Ok((h, img)) = self.rx.try_recv() {
            let tex = img.map(|i| ctx.load_texture(format!("avatar-{h}"), i, Default::default()));
            textures.insert(h, tex);
        }
        if let Some(t) = textures.get(hash) {
            return t.as_ref().map(|t| t.id());
        }
        textures.insert(hash.clone(), None);
        let (Some(from), account, hash) = (self.endpoint.clone(), *account, hash.clone()) else {
            return None;
        };
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let img = avatar::fetch(&from, account, &hash)
                .and_then(|p| std::fs::read(p).map_err(|e| e.to_string()))
                .and_then(|b| {
                    let side = proto::files::AVATAR_SIDE;
                    tuffcord::images::decode_fit(&b, side, side)
                });
            if let Err(e) = &img {
                tuffcord::applog::warn(format!("Couldn't load a profile picture: {e}"));
            }
            let _ = tx.send((hash, img.ok()));
            ctx.request_repaint();
        });
        None
    }
}

impl App {
    /// Settings > Account: your picture, with Choose and Remove.
    pub(crate) fn avatar_settings(&mut self, ui: &mut egui::Ui) {
        let Some(me) = self.acct.me.as_ref().map(|m| m.name.clone()) else {
            return;
        };
        let av = &mut self.avatars;
        if av.endpoint.is_none() {
            return;
        }
        if let Some(r) = av.done.as_ref().and_then(|rx| rx.try_recv().ok()) {
            av.done = None;
            av.status = match r {
                Ok("") => None,
                r => Some((false, r.map(str::to_string).unwrap_or_else(|e| e))),
            };
        }
        let busy = av.status.as_ref().is_some_and(|s| s.0);
        let has = av.by_name.contains_key(&me);
        ui.label(
            RichText::new("Profile picture")
                .size(12.5)
                .color(pal().faint),
        );
        let mut choose = false;
        let mut remove = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::hover());
            let pic = av.pic(ui.ctx(), &me);
            paint_avatar(ui.painter(), rect.center(), 22.0, &me, pic, false, pal().bg);
            ui.add_space(6.0);
            choose = ui
                .add_enabled(!busy, egui::Button::new("Choose picture…"))
                .clicked();
            remove = has && ui.add_enabled(!busy, egui::Button::new("Remove")).clicked();
        });
        if let Some((_, text)) = &av.status {
            ui.label(RichText::new(text).size(12.0).color(pal().faint));
        }
        if choose || remove {
            let to = av.endpoint.clone().unwrap();
            let (tx, rx) = channel();
            av.done = Some(rx);
            av.status = Some((
                true,
                if remove {
                    "Removing…"
                } else {
                    "Choose a picture…"
                }
                .into(),
            ));
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let r = if remove {
                    avatar::send(&to, None).map(|_| "Picture removed.")
                } else {
                    match tuffcord::images::pick_file(false) {
                        None => Ok(""),
                        Some(path) => avatar::make(&path)
                            .and_then(|png| avatar::send(&to, Some(&png)))
                            .map(|_| "Picture changed."),
                    }
                };
                let _ = tx.send(r.map_err(|e| format!("Couldn't change your picture: {e}")));
                ctx.request_repaint();
            });
        }
        ui.add_space(8.0);
    }
}
