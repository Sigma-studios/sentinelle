//! Time-series charts, meters and the per-core strip, painted directly.

use super::theme::*;
use crate::core::History;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

pub struct Chart<'a> {
    pub data: &'a History,
    pub color: Color32,
    pub range: (f32, f32),
    /// Values at which to draw hairlines; the first one is labeled.
    pub grid: &'a [f32],
    pub fill: bool,
    pub label_grid: bool,
    pub format: &'a dyn Fn(f32) -> String,
    pub interval: f32,
}

impl<'a> Chart<'a> {
    pub fn new(data: &'a History, color: Color32, format: &'a dyn Fn(f32) -> String) -> Self {
        Self { data, color, range: (0.0, 100.0), grid: &[], fill: true, label_grid: true, format, interval: 1.0 }
    }

    pub fn range(mut self, lo: f32, hi: f32) -> Self {
        self.range = (lo, hi);
        self
    }

    pub fn grid(mut self, grid: &'a [f32]) -> Self {
        self.grid = grid;
        self
    }

    /// Sparklines are too small for an axis label; hover shows values.
    pub fn sparkline(mut self) -> Self {
        self.label_grid = false;
        self
    }

    pub fn line_only(mut self) -> Self {
        self.fill = false;
        self
    }

    pub fn interval(mut self, s: f32) -> Self {
        self.interval = s;
        self
    }

    pub fn show(self, ui: &mut Ui, size: egui::Vec2) -> Response {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let painter = ui.painter_at(rect.expand(8.0));
        let (lo, hi) = self.range;
        let span = (hi - lo).max(1e-6);
        let y_of = |v: f32| rect.bottom() - ((v - lo) / span).clamp(0.0, 1.0) * rect.height();

        for (i, &g) in self.grid.iter().enumerate() {
            let y = y_of(g).round() + 0.5;
            painter.hline(rect.x_range(), y, Stroke::new(1.0, GRID));
            if i == 0 && self.label_grid {
                painter.text(
                    pos2(rect.left() + 2.0, y + 3.0),
                    Align2::LEFT_TOP,
                    (self.format)(g),
                    font(10.0),
                    FAINT,
                );
            }
        }
        painter.hline(rect.x_range(), rect.bottom().round() - 0.5, Stroke::new(1.0, GRID));

        let cap = self.data.capacity().max(2);
        let step = rect.width() / (cap - 1) as f32;
        let n = self.data.len();
        let x_of = |i: usize| rect.right() - (n - 1 - i) as f32 * step;

        // Draw each run of finite samples; NaN marks a gap (e.g. lost ping).
        let mut run: Vec<Pos2> = Vec::with_capacity(n);
        let flush = |run: &mut Vec<Pos2>| {
            if run.len() >= 2 {
                let pts = smooth(run, rect.top(), rect.bottom());
                if self.fill {
                    painter.add(area_mesh(&pts, rect, self.color));
                }
                painter.add(Shape::line(pts, Stroke::new(2.0, self.color)));
            } else if run.len() == 1 {
                painter.circle_filled(run[0], 1.5, self.color);
            }
            run.clear();
        };
        for (i, v) in self.data.iter().enumerate() {
            if v.is_finite() {
                run.push(pos2(x_of(i), y_of(v)));
            } else {
                flush(&mut run);
                let x = x_of(i);
                painter.line_segment([pos2(x, rect.bottom() - 5.0), pos2(x, rect.bottom())], Stroke::new(1.5, MUTED));
            }
        }
        flush(&mut run);

        // End marker with a surface ring.
        if let Some(v) = self.data.last().filter(|v| v.is_finite()) {
            let p = pos2(rect.right(), y_of(v));
            painter.circle_filled(p, 5.5, SURFACE);
            painter.circle_filled(p, 3.5, self.color);
        }

        // Hover: crosshair, marker and value.
        if let Some(pos) = resp.hover_pos() {
            if n > 0 {
                let i = (((pos.x - x_of(0)) / step).round().max(0.0) as usize).min(n - 1);
                let x = x_of(i);
                painter.vline(x, rect.y_range(), Stroke::new(1.0, FAINT));
                let v = self.data.get(i).unwrap_or(f32::NAN);
                if v.is_finite() {
                    let p = pos2(x, y_of(v));
                    painter.circle_filled(p, 5.5, SURFACE);
                    painter.circle_filled(p, 3.5, self.color);
                }
                let ago = ((n - 1 - i) as f32 * self.interval).round() as u32;
                let value = if v.is_finite() { (self.format)(v) } else { "—".into() };
                let when = if ago == 0 { "now".to_string() } else { format!("{ago}s ago") };
                tooltip(ui, &painter, rect, x, &value, &when);
            }
        }
        resp
    }
}

fn tooltip(ui: &Ui, painter: &egui::Painter, rect: Rect, x: f32, value: &str, when: &str) {
    let v = ui.fonts_mut(|f| f.layout_no_wrap(value.to_string(), strong(12.0), TEXT));
    let w = ui.fonts_mut(|f| f.layout_no_wrap(when.to_string(), font(11.0), MUTED));
    let size = vec2(v.size().x + w.size().x + 22.0, v.size().y.max(w.size().y) + 8.0);
    let left = if x + 10.0 + size.x <= rect.right() { x + 8.0 } else { x - 8.0 - size.x };
    let r = Rect::from_min_size(pos2(left, rect.top() + 2.0), size);
    painter.rect(r, CornerRadius::same(5), RAISED, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    let cy = r.center().y;
    painter.galley(pos2(r.left() + 8.0, cy - v.size().y / 2.0), v.clone(), TEXT);
    painter.galley(pos2(r.left() + 14.0 + v.size().x, cy - w.size().y / 2.0), w, MUTED);
}

/// Monotone cubic interpolation (Fritsch–Carlson): smooth, but never
/// overshoots the data, so a 100% sample never draws above 100%.
fn smooth(p: &[Pos2], top: f32, bottom: f32) -> Vec<Pos2> {
    let n = p.len();
    if n < 3 {
        return p.to_vec();
    }
    let h: Vec<f32> = (0..n - 1).map(|k| (p[k + 1].x - p[k].x).max(1e-3)).collect();
    let d: Vec<f32> = (0..n - 1).map(|k| (p[k + 1].y - p[k].y) / h[k]).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for k in 1..n - 1 {
        m[k] = if d[k - 1] * d[k] <= 0.0 { 0.0 } else { (d[k - 1] + d[k]) / 2.0 };
    }
    for k in 0..n - 1 {
        if d[k] == 0.0 {
            m[k] = 0.0;
            m[k + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[k] = t * a * d[k];
            m[k + 1] = t * b * d[k];
        }
    }
    const SUB: usize = 5;
    let mut out = Vec::with_capacity((n - 1) * SUB + 1);
    for k in 0..n - 1 {
        for j in 0..SUB {
            let t = j as f32 / SUB as f32;
            let (t2, t3) = (t * t, t * t * t);
            let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[k].y
                + (t3 - 2.0 * t2 + t) * h[k] * m[k]
                + (-2.0 * t3 + 3.0 * t2) * p[k + 1].y
                + (t3 - t2) * h[k] * m[k + 1];
            out.push(pos2(p[k].x + t * h[k], y.clamp(top, bottom)));
        }
    }
    out.push(p[n - 1]);
    out
}

/// Area under the curve with a vertical wash that fades into the baseline.
fn area_mesh(pts: &[Pos2], rect: Rect, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    let alpha = |y: f32| 0.30 * ((rect.bottom() - y) / rect.height()).clamp(0.0, 1.0) + 0.02;
    for (i, p) in pts.iter().enumerate() {
        mesh.colored_vertex(*p, color.gamma_multiply(alpha(p.y)));
        mesh.colored_vertex(pos2(p.x, rect.bottom()), color.gamma_multiply(0.02));
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    mesh
}

/// Horizontal meter: a track in the surface family and a fill.
pub fn meter(ui: &mut Ui, width: f32, frac: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), TRACK);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(6.0), rect.height()));
        p.rect_filled(fill, CornerRadius::same(3), color);
    }
    resp
}

/// One column per logical core, growing from a shared baseline.
pub fn core_strip(ui: &mut Ui, size: egui::Vec2, cores: &[f32]) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if cores.is_empty() {
        return resp;
    }
    let p = ui.painter();
    let n = cores.len() as f32;
    let gap = 2.0;
    let w = ((rect.width() - gap * (n - 1.0)) / n).min(24.0);
    let total_w = w * n + gap * (n - 1.0);
    let mut x = rect.left() + (rect.width() - total_w) / 2.0;
    let hovered = resp.hover_pos();
    for (i, &c) in cores.iter().enumerate() {
        let col = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        p.rect_filled(col, CornerRadius::same(2), TRACK);
        let h = (c / 100.0).clamp(0.0, 1.0) * rect.height();
        if h > 0.5 {
            let bar = Rect::from_min_max(pos2(x, rect.bottom() - h.max(2.0)), col.max);
            let r = (2.0f32).min(h / 2.0) as u8;
            p.rect_filled(bar, CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, LOAD);
        }
        if let Some(hp) = hovered {
            if col.expand2(vec2(gap / 2.0, 0.0)).contains(hp) {
                p.rect_stroke(col.expand(1.0), CornerRadius::same(3), Stroke::new(1.0, TEXT_2), egui::StrokeKind::Outside);
                let text = format!("Thread {i}  {c:.0}%");
                let g = ui.fonts_mut(|f| f.layout_no_wrap(text, font(11.0), TEXT));
                let tw = g.size().x + 14.0;
                let left = (col.center().x - tw / 2.0).clamp(rect.left(), rect.right() - tw);
                let tr = Rect::from_min_size(pos2(left, rect.top() - 24.0), vec2(tw, 20.0));
                ui.painter().with_clip_rect(ui.clip_rect()).rect(tr, CornerRadius::same(5), RAISED, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
                ui.painter().with_clip_rect(ui.clip_rect()).galley(pos2(tr.left() + 7.0, tr.center().y - g.size().y / 2.0), g, TEXT);
            }
        }
        x += w + gap;
    }
    resp
}

/// Rounded "nice" ceiling for auto-scaled axes: 1, 2, 5 × 10^n.
pub fn nice_ceil(v: f32) -> f32 {
    if v <= 0.0 {
        return 1.0;
    }
    let e = 10f32.powf(v.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if v <= m * e {
            return m * e;
        }
    }
    10.0 * e
}

/// Axis range for temperatures: tight around the data, at least 20° tall.
pub fn temp_range(h: &History) -> (f32, f32) {
    let (mn, mx) = (h.min().unwrap_or(40.0), h.max().unwrap_or(60.0));
    let lo = ((mn - 5.0) / 10.0).floor() * 10.0;
    let mut hi = ((mx + 5.0) / 10.0).ceil() * 10.0;
    if hi - lo < 20.0 {
        hi = lo + 20.0;
    }
    (lo.max(0.0), hi)
}
