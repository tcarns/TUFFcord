//! Small drawing helpers shared by the screens: colors, avatars, buttons, swatches, time labels.

use crate::theme::{self, pal};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke};

// ---------------------------------------------------------------- drawing helpers

/// The draggable edge of a side panel (`panel`, on the left of the window or
/// the right): a thin line lights up on hover, dragging sets `width` within
/// `min..=max`. Returns true when a drag ends. Call after all panels are
/// shown, so the edge wins over what's under it.
pub(crate) fn panel_edge(
    ctx: &egui::Context,
    panel: egui::Rect,
    left: bool,
    width: &mut f32,
    (min, max): (f32, f32),
) -> bool {
    let id = egui::Id::new(("panel_edge", left));
    let x = if left { panel.right() } else { panel.left() };
    let ui = egui::Ui::new(
        ctx.clone(),
        id,
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.screen_rect()),
    );
    let grab = egui::Rect::from_x_y_ranges(x - 3.0..=x + 3.0, panel.y_range());
    let resp = ui.interact(grab, id.with("grab"), Sense::drag());
    if resp.hovered() || resp.dragged() {
        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        let color = if resp.dragged() {
            pal().accent
        } else {
            theme::mix(pal().line, pal().accent, 0.5)
        };
        ui.painter()
            .vline(x, panel.y_range(), Stroke::new(2.0_f32, color));
    }
    if resp.dragged() {
        if let Some(p) = ctx.pointer_latest_pos() {
            let w = if left {
                p.x - panel.left()
            } else {
                panel.right() - p.x
            };
            *width = w.clamp(min, max.max(min)).round();
        }
    }
    resp.drag_stopped()
}

pub(crate) fn hsl(h: f32, s: f32, l: f32) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h / 60.0) % 6.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub(crate) fn avatar_color(name: &str) -> Color32 {
    const HUES: [f32; 8] = [280.0, 330.0, 15.0, 42.0, 150.0, 190.0, 225.0, 255.0];
    let mut h: u32 = 0;
    for c in name.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as u32);
    }
    hsl(HUES[(h % 8) as usize], 0.45, 0.72)
}

pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |w: &str| {
        w.chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default()
    };
    match words.as_slice() {
        [a, b, ..] => first(a) + &first(b),
        [a] => first(a),
        [] => "?".into(),
    }
}

/// A small trash can: lid with a handle, and a tapered bin with two ribs.
pub(crate) fn paint_trash(p: &egui::Painter, c: egui::Pos2, color: Color32) {
    let s = Stroke::new(1.5_f32, color);
    let v = |x: f32, y: f32| c + egui::vec2(x, y);
    // Lid and handle.
    p.line_segment([v(-6.0, -4.0), v(6.0, -4.0)], s);
    p.line_segment([v(-2.0, -4.0), v(-2.0, -6.0)], s);
    p.line_segment([v(-2.0, -6.0), v(2.0, -6.0)], s);
    p.line_segment([v(2.0, -6.0), v(2.0, -4.0)], s);
    // Bin.
    p.add(egui::Shape::closed_line(
        vec![v(-4.6, -2.0), v(4.6, -2.0), v(3.6, 6.5), v(-3.6, 6.5)],
        s,
    ));
    p.line_segment([v(-1.4, 0.2), v(-1.2, 4.4)], s);
    p.line_segment([v(1.4, 0.2), v(1.2, 4.4)], s);
}

/// Round avatar: their profile picture, else initials. Speaking adds the
/// amber glow ring.
pub(crate) fn paint_avatar(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    name: &str,
    pic: Option<egui::TextureId>,
    speaking: bool,
    ring_bg: Color32,
) {
    if speaking {
        painter.circle_filled(center, radius + 6.0, theme::with_alpha(pal().accent, 46));
        painter.circle_filled(center, radius + 4.0, pal().accent);
        painter.circle_filled(center, radius + 2.0, ring_bg);
    }
    if let Some(tex) = pic {
        // The picture on a circle: a fan of triangles from the middle.
        let mut mesh = egui::Mesh::with_texture(tex);
        let uv = |x: f32, y: f32| egui::pos2(0.5 + 0.5 * x, 0.5 + 0.5 * y);
        mesh.vertices.push(egui::epaint::Vertex {
            pos: center,
            uv: uv(0.0, 0.0),
            color: Color32::WHITE,
        });
        const STEPS: u32 = 32;
        for i in 0..=STEPS {
            let a = i as f32 / STEPS as f32 * std::f32::consts::TAU;
            let (y, x) = a.sin_cos();
            mesh.vertices.push(egui::epaint::Vertex {
                pos: center + egui::vec2(x, y) * radius,
                uv: uv(x, y),
                color: Color32::WHITE,
            });
            if i > 0 {
                mesh.add_triangle(0, i, i + 1);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        return;
    }
    painter.circle_filled(center, radius, avatar_color(name));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initials(name),
        FontId::proportional(radius * 0.85),
        Color32::from_rgb(26, 19, 32),
    );
}

/// A square icon button; `crossed` draws a red slash over it (muted, deafened).
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    crossed: bool,
    color: Color32,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(9), pal().raised_2);
    }
    paint_glyph(p, rect, glyph, crossed, color);
    resp.on_hover_text(tip)
}

/// A button in the bar under the channel list (mute, deafen, settings): a
/// soft highlight fades in under the pointer and the icon brightens. Tinted
/// from the bar's own color so it shows in every theme.
pub(crate) fn bar_button(
    ui: &mut egui::Ui,
    glyph: &str,
    crossed: bool,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::click());
    let t = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if t > 0.0 {
        let strength = if resp.is_pointer_button_down_on() {
            0.16
        } else {
            0.10
        };
        let fill = theme::mix(pal().me_bg, pal().text, strength * t);
        p.rect_filled(rect.shrink(1.0), CornerRadius::same(8), fill);
    }
    let color = theme::mix(pal().muted, pal().text, t);
    paint_glyph(p, rect, glyph, crossed, color);
    resp.on_hover_text(tip)
}

/// An icon centered in `rect`, red with a slash through it when `crossed`.
fn paint_glyph(p: &egui::Painter, rect: egui::Rect, glyph: &str, crossed: bool, color: Color32) {
    let c = if crossed { pal().red } else { color };
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(17.0),
        c,
    );
    if crossed {
        p.line_segment(
            [
                rect.center() + egui::vec2(-9.0, -9.0),
                rect.center() + egui::vec2(9.0, 9.0),
            ],
            Stroke::new(2.2_f32, pal().red),
        );
    }
}

/// Paper-plane style send arrow, drawn so it doesn't depend on font glyphs.
pub(crate) fn send_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(9), pal().raised_2);
    }
    let c = rect.center();
    let pts = vec![
        c + egui::vec2(-8.0, -7.0),
        c + egui::vec2(9.0, 0.0),
        c + egui::vec2(-8.0, 7.0),
        c + egui::vec2(-5.0, 0.0),
    ];
    p.add(egui::Shape::convex_polygon(pts, pal().accent, Stroke::NONE));
    resp.on_hover_text("Send (Enter)")
}

/// Round "+" button for attaching an image. Drawn by hand so it doesn't depend on font glyphs.
pub(crate) fn attach_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(36.0, 36.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let p = ui.painter();
    let c = rect.center();
    let color = if !enabled {
        theme::mix(pal().muted, pal().raised, 0.5)
    } else if resp.hovered() {
        pal().text
    } else {
        pal().muted
    };
    p.circle_filled(
        c,
        11.0,
        if resp.hovered() && enabled {
            pal().raised_2
        } else {
            Color32::TRANSPARENT
        },
    );
    p.circle_stroke(c, 10.0, Stroke::new(1.6_f32, color));
    p.line_segment(
        [c + egui::vec2(-4.5, 0.0), c + egui::vec2(4.5, 0.0)],
        Stroke::new(1.8_f32, color),
    );
    p.line_segment(
        [c + egui::vec2(0.0, -4.5), c + egui::vec2(0.0, 4.5)],
        Stroke::new(1.8_f32, color),
    );
    resp
}

pub(crate) fn close_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), pal().line);
    }
    let c = rect.center();
    let s = Stroke::new(1.6_f32, pal().muted);
    p.line_segment([c + egui::vec2(-4.5, -4.5), c + egui::vec2(4.5, 4.5)], s);
    p.line_segment([c + egui::vec2(4.5, -4.5), c + egui::vec2(-4.5, 4.5)], s);
    resp.on_hover_text("Dismiss")
}

/// A small preview card for a theme: sidebar and chat colors, accent dot, sample text lines.
pub(crate) fn theme_swatch(
    ui: &mut egui::Ui,
    t: &theme::Palette,
    selected: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(58.0, 62.0), Sense::click());
    let p = ui.painter();
    let card = egui::Rect::from_min_size(rect.min, egui::vec2(58.0, 40.0));
    let r = 8u8;
    p.rect_filled(card, CornerRadius::same(r), t.bg_deep);
    let right = egui::Rect::from_min_max(egui::pos2(card.left() + 20.0, card.top()), card.max);
    p.rect_filled(
        right,
        CornerRadius {
            nw: 0,
            sw: 0,
            ne: r,
            se: r,
        },
        t.bg,
    );
    p.circle_filled(card.left_center() + egui::vec2(11.0, 0.0), 5.5, t.accent);
    let line = |y: f32, w: f32, c: Color32| {
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(right.left() + 7.0, card.top() + y),
                egui::vec2(w, 3.0),
            ),
            CornerRadius::same(1),
            c,
        );
    };
    line(10.0, 24.0, t.text);
    line(18.0, 18.0, t.muted);
    line(26.0, 20.0, t.text);
    let border = if selected {
        Stroke::new(2.0_f32, pal().accent)
    } else if resp.hovered() {
        Stroke::new(1.0_f32, pal().muted)
    } else {
        Stroke::new(1.0_f32, pal().line)
    };
    p.rect_stroke(
        card,
        CornerRadius::same(r),
        border,
        egui::StrokeKind::Outside,
    );
    p.text(
        egui::pos2(card.center().x, card.bottom() + 12.0),
        Align2::CENTER_CENTER,
        t.label,
        FontId::proportional(12.0),
        if selected { pal().text } else { pal().muted },
    );
    resp.on_hover_text(t.label)
}

pub(crate) fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(
        RichText::new(text)
            .color(pal().accent_ink)
            .strong()
            .size(15.0),
    )
    .fill(if enabled {
        pal().accent
    } else {
        theme::mix(pal().accent, pal().bg, 0.45)
    })
    .corner_radius(CornerRadius::same(10))
    .min_size(egui::vec2(ui.available_width(), 40.0));
    ui.add_enabled(enabled, btn)
}

pub(crate) fn day_label(ts: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(d) = Local.timestamp_millis_opt(ts as i64).single() else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let day = d.date_naive();
    if day == today {
        "Today".into()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".into()
    } else if d.format("%Y").to_string() == Local::now().format("%Y").to_string() {
        d.format("%A, %B %-d").to_string()
    } else {
        d.format("%B %-d, %Y").to_string()
    }
}

pub(crate) fn time_label(ts: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ts as i64)
        .single()
        .map(|d| d.format("%-I:%M %p").to_string())
        .unwrap_or_default()
}
