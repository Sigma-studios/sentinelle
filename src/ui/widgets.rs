//! Small building blocks shared by every section.

use super::theme::*;
use eframe::egui::{self, pos2, text::LayoutJob, vec2, Align, Color32, Layout, Margin, Rect, Response, RichText, Sense, Stroke, TextFormat, Ui};

/// A card: the one container shape used everywhere.
pub fn card<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(RADIUS)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            let w = (width - 34.0).max(10.0);
            ui.set_width(w);
            add(ui)
        })
        .inner
}

/// Run `add` in a region of exactly `height`, so sibling cards line up.
pub fn fixed<R>(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    add(&mut child)
}

/// Eyebrow title on the left, muted detail on the right.
pub fn card_title(ui: &mut Ui, title: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(eyebrow(title, MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(detail).font(font(12.0)).color(TEXT_2)).truncate());
        });
    });
}

/// Big number with a smaller unit, bottom aligned.
pub fn hero(value: &str, unit: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(value, 0.0, TextFormat { font_id: strong(30.0), color: TEXT, valign: Align::BOTTOM, ..Default::default() });
    job.append(
        unit,
        2.0,
        TextFormat { font_id: font(15.0), color: MUTED, valign: Align::BOTTOM, ..Default::default() },
    );
    job
}

/// A colored mark followed by a text label: the identity key for a series.
pub fn key(ui: &mut Ui, color: Color32, label: &str) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 12.0), Sense::hover());
        ui.painter().line_segment(
            [pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)],
            Stroke::new(2.5, color),
        );
        ui.label(RichText::new(label).font(font(12.0)).color(MUTED));
    })
    .response
}

/// Value text with an optional status dot + label (never color alone).
/// Laid out right-to-left: call inside a right_to_left layout.
pub fn value_with_status(ui: &mut Ui, value: &str, status: Option<(Color32, &str)>) {
    ui.label(RichText::new(value).font(strong(13.0)).color(TEXT));
    if let Some((c, label)) = status {
        status_badge(ui, c, label);
    }
}

/// A status dot and its label as one unit.
pub fn status_badge(ui: &mut Ui, color: Color32, label: &str) -> Response {
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font(12.0), TEXT_2));
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 14.0, g.size().y.max(14.0)), Sense::hover());
    ui.painter().circle_filled(pos2(r.left() + 3.5, r.center().y), 3.5, color);
    ui.painter().galley(pos2(r.left() + 14.0, r.center().y - g.size().y / 2.0), g, TEXT_2);
    resp
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).font(font(12.0)).color(MUTED)
}

pub fn secondary(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).font(font(12.0)).color(TEXT_2)
}

/// Section heading above a group of cards.
pub fn section(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui)) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(eyebrow(title, FAINT));
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
    ui.add_space(2.0);
}

/// A quiet text button that only shows a background on hover.
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font(12.0), TEXT_2));
    let size = galley.size() + vec2(16.0, 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let color = if resp.hovered() {
        ui.painter().rect_filled(rect, 6, SURFACE_HOVER);
        TEXT
    } else {
        TEXT_2
    };
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Wi-Fi glyph: three arcs and a dot; lit arcs show signal strength.
pub fn wifi_icon(ui: &mut Ui, signal: Option<u8>) {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    let p = ui.painter();
    let c = pos2(rect.center().x, rect.bottom() - 3.0);
    let lit = match signal {
        Some(s) if s >= 66 => 3,
        Some(s) if s >= 33 => 2,
        Some(_) => 1,
        None => 3,
    };
    p.circle_filled(c, 2.0, TEXT);
    for i in 0..3 {
        let r = 6.0 + i as f32 * 5.0;
        let color = if i < lit { TEXT } else { FAINT };
        let pts: Vec<_> = (0..=12)
            .map(|k| {
                let a = std::f32::consts::PI * (1.25 + 0.5 * k as f32 / 12.0);
                pos2(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.8, color)));
    }
}

/// Ethernet glyph: a small RJ45 outline.
pub fn wired_icon(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    let p = ui.painter();
    let body = Rect::from_center_size(rect.center(), vec2(16.0, 13.0));
    p.rect_stroke(body, 2, Stroke::new(1.8, TEXT), egui::StrokeKind::Middle);
    let tab = Rect::from_center_size(pos2(body.center().x, body.bottom() + 1.5), vec2(7.0, 4.0));
    p.rect_filled(tab, 1, TEXT);
    for i in 0..4 {
        let x = body.left() + 4.0 + i as f32 * 2.7;
        p.line_segment([pos2(x, body.top() + 3.0), pos2(x, body.top() + 6.0)], Stroke::new(1.2, TEXT));
    }
}

/// Diagonal hatching: the texture for "space we can't attribute".
pub fn hatch(p: &egui::Painter, r: Rect, color: Color32) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    p.rect_filled(r, 0, color.gamma_multiply(0.25));
    let step = 5.0;
    let mut x = r.left() - r.height();
    while x < r.right() {
        p.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], Stroke::new(1.0, color));
        x += step;
    }
}
