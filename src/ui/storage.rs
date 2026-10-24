//! Disk usage bars split into the paths that take the space.

use super::theme::*;
use super::widgets::*;
use crate::core::{self, Command, DiskSegment, DiskSource, DiskStats, SegmentKind};
use eframe::egui::{self, pos2, vec2, Align, Color32, CornerRadius, CursorIcon, Layout, Rect, RichText, Sense, Stroke, Ui};

#[derive(Default)]
pub struct StorageState {
    /// (disk id, segment index) under the pointer last frame.
    hovered: Option<(String, usize)>,
}

pub fn show(ui: &mut Ui, disks: &[DiskStats], state: &mut StorageState, send: &dyn Fn(Command)) {
    let scanning = disks.iter().any(|d| d.scanning.is_some());
    section(ui, "Storage", |ui| {
        if !scanning && ghost_button(ui, "Rescan").on_hover_text("Measure folder sizes again").clicked() {
            send(Command::RescanDisks);
        }
        if let Some((color, label, tip)) = live_state(disks) {
            status_badge(ui, color, label).on_hover_text(tip);
        } else if let Some(age) = disks.iter().filter_map(|d| d.scanned_ago).max() {
            ui.label(muted(format!("measured {} ago", duration(age))));
        }
    });
    let width = ui.available_width();
    card(ui, width, |ui| {
        let mut next_hover = None;
        for (i, d) in disks.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.0);
                let w = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
                ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, BORDER));
                ui.add_space(10.0);
            }
            if let Some(h) = disk(ui, d, state.hovered.as_ref().filter(|(id, _)| *id == d.id).map(|h| h.1)) {
                next_hover = Some((d.id.clone(), h));
            }
        }
        if disks.is_empty() {
            ui.label(muted("No filesystems found"));
        }
        state.hovered = next_hover;
    });
}

/// When répertoire measures the disks: how current the numbers are, as a
/// status dot with its label and an explanation.
fn live_state(disks: &[DiskStats]) -> Option<(Color32, &'static str, &'static str)> {
    let sources: Vec<DiskSource> = disks.iter().map(|d| d.source).filter(|s| *s != DiskSource::Scan).collect();
    if sources.is_empty() {
        return None;
    }
    Some(if sources.iter().any(|s| matches!(s, DiskSource::Stale { .. })) {
        (WARNING, "catching up", "répertoire missed some changes and is measuring again; sizes may be off until it's done")
    } else if sources.iter().any(|s| matches!(s, DiskSource::Reconciling { .. } | DiskSource::Indexing { .. })) {
        (MUTED, "checking", "répertoire is checking its saved sizes against the disk; they update as it goes")
    } else {
        (GOOD, "live", "Sizes follow every change as it happens (répertoire)")
    })
}

/// What a disk's own header says about where its numbers stand.
fn source_note(d: &DiskStats) -> Option<String> {
    let pct = |p: f32| (p * 100.0).round() as u32;
    match d.source {
        DiskSource::Indexing { progress } => Some(format!("indexing… {}% ·", pct(progress))),
        DiskSource::Reconciling { progress } => Some(format!("checking saved sizes… {}% ·", pct(progress))),
        DiskSource::Stale { progress } => Some(format!("missed changes, measuring again… {}% ·", pct(progress))),
        DiskSource::Live | DiskSource::Scan => None,
    }
}

fn segment_color(segments: &[DiskSegment], i: usize) -> Color32 {
    match segments[i].kind {
        SegmentKind::Other => OTHER,
        SegmentKind::Hidden => FAINT,
        _ => {
            // Rank among real paths, largest first.
            let rank = segments[..i].iter().filter(|s| matches!(s.kind, SegmentKind::Dir | SegmentKind::Rest)).count();
            RAMP[rank.min(RAMP.len() - 1)]
        }
    }
}

fn pretty_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().into_owned();
        if home.len() > 1 {
            if s == home {
                return "~".into();
            }
            if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
                return format!("~/{rest}");
            }
        }
    }
    s
}

fn segment_label(s: &DiskSegment) -> (String, Option<&'static str>) {
    match s.kind {
        SegmentKind::Dir => (pretty_path(&s.path), None),
        SegmentKind::Rest => (pretty_path(&s.path), Some("other files")),
        SegmentKind::Other => ("Smaller folders".into(), None),
        SegmentKind::Hidden => ("Not readable".into(), Some("system, other users")),
    }
}

fn clickable(s: &DiskSegment) -> bool {
    matches!(s.kind, SegmentKind::Dir | SegmentKind::Rest)
}

/// Returns the hovered segment index.
fn disk(ui: &mut Ui, d: &DiskStats, highlighted: Option<usize>) -> Option<usize> {
    let mut hovered = None;
    let free = d.total.saturating_sub(d.used);

    ui.horizontal(|ui| {
        let mounts: Vec<String> = d.mount_points.iter().map(|m| m.to_string_lossy().into_owned()).collect();
        ui.label(RichText::new(mounts.join("   ")).font(strong(14.0)).color(TEXT));
        ui.label(muted(format!("{} · {}", d.device, d.fs_type)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(muted(format!("{} free", bytes(free as f64))));
            ui.label(secondary(format!("{} of {}", bytes(d.used as f64), bytes(d.total as f64))));
            if let DiskSource::Stale { .. } = d.source {
                ui.label(muted("·"));
                status_badge(ui, WARNING, source_note(d).unwrap_or_default().trim_end_matches(" ·"));
            } else if let Some(note) = source_note(d) {
                ui.label(muted(note));
            } else if let Some(seen) = d.scanning {
                ui.label(muted(format!("measuring… {} so far ·", bytes(seen as f64))));
            }
        });
    });
    ui.add_space(4.0);

    // ---- the bar
    let w = ui.available_width();
    let (bar, bar_resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::same(5), TRACK);
    let total = d.total.max(1) as f32;
    let gap = 2.0;
    let pointer = bar_resp.hover_pos();

    if d.segments.is_empty() {
        let used_w = bar.width() * d.used as f32 / total;
        let r = Rect::from_min_size(bar.min, vec2(used_w, bar.height()));
        p.rect_filled(r, CornerRadius { nw: 5, sw: 5, ne: 0, se: 0 }, OTHER);
    } else {
        let mut x = bar.left();
        let n = d.segments.len();
        let used_w = bar.width() * d.used as f32 / total;
        let mut rects = Vec::with_capacity(n);
        for s in &d.segments {
            let sw = used_w * s.size as f32 / d.used.max(1) as f32;
            let r = Rect::from_min_max(pos2(x, bar.top()), pos2((x + sw - gap).max(x + 1.0), bar.bottom()));
            rects.push(r);
            x += sw;
        }
        for (i, (s, r)) in d.segments.iter().zip(&rects).enumerate() {
            if pointer.is_some_and(|pp| r.expand2(vec2(gap / 2.0, 0.0)).contains(pp)) {
                hovered = Some(i);
            }
            let active = hovered.or(highlighted);
            let mut c = segment_color(&d.segments, i);
            if active.is_some_and(|a| a != i) {
                c = c.gamma_multiply(0.35);
            }
            let cr = CornerRadius {
                nw: if i == 0 { 5 } else { 0 },
                sw: if i == 0 { 5 } else { 0 },
                ne: 0,
                se: 0,
            };
            if s.kind == SegmentKind::Hidden {
                hatch(p, *r, c);
            } else {
                p.rect_filled(*r, cr, c);
            }
        }
        if let Some(i) = hovered {
            let s = &d.segments[i];
            let (label, note) = segment_label(s);
            let pct = s.size as f32 / d.used.max(1) as f32 * 100.0;
            let resp = bar_resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(&label).font(strong(13.0)).color(TEXT));
                ui.label(secondary(format!("{} · {pct:.0}% of used space", bytes(s.size as f64))));
                if let Some(n) = note {
                    ui.label(muted(n));
                }
                if clickable(s) {
                    ui.label(muted("Click to open"));
                }
            });
            if clickable(s) {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if resp.clicked() {
                    core::open_in_file_manager(&s.path);
                }
            }
        }
    }

    // ---- legend
    ui.add_space(8.0);
    if d.segments.is_empty() {
        ui.label(muted(if d.scanning.is_some() { "Measuring folders…" } else { "Folder sizes not measured yet" }));
        return hovered;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, s) in d.segments.iter().enumerate() {
            let active = hovered.or(highlighted) == Some(i);
            if legend_chip(ui, s, segment_color(&d.segments, i), active, d.used) {
                hovered = Some(i);
            }
        }
    });
    hovered
}

/// Returns true while hovered. Clicking opens the folder.
fn legend_chip(ui: &mut Ui, s: &DiskSegment, color: Color32, active: bool, used: u64) -> bool {
    let (label, note) = segment_label(s);
    let size = bytes(s.size as f64);
    let lg = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font(12.0), TEXT));
    let sg = ui.fonts_mut(|f| f.layout_no_wrap(size.clone(), font(12.0), MUTED));
    let h = 24.0;
    let w = 10.0 + 8.0 + 7.0 + lg.size().x + 7.0 + sg.size().x + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let p = ui.painter();
    if active || resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), SURFACE_HOVER);
    }
    let sw = Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(8.0, 8.0));
    if s.kind == SegmentKind::Hidden {
        hatch(p, sw, color);
    } else {
        p.rect_filled(sw, CornerRadius::same(2), color);
    }
    let tx = sw.right() + 7.0;
    p.galley(pos2(tx, rect.center().y - lg.size().y / 2.0), lg.clone(), TEXT);
    p.galley(pos2(tx + lg.size().x + 7.0, rect.center().y - sg.size().y / 2.0), sg, MUTED);

    let pct = s.size as f32 / used.max(1) as f32 * 100.0;
    let resp = resp.on_hover_ui(|ui| {
        ui.label(secondary(format!("{size} · {pct:.0}% of used space")));
        if let Some(n) = note {
            ui.label(muted(n));
        }
    });
    if clickable(s) {
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            core::open_in_file_manager(&s.path);
        }
        resp.hovered()
    } else {
        resp.hovered()
    }
}

/// Diagonal hatching: the texture for "space we can't attribute".
fn hatch(p: &egui::Painter, r: Rect, color: Color32) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    p.rect_filled(r, 0, color.gamma_multiply(0.25));
    let step = 5.0;
    let mut x = r.left() - r.height();
    while x < r.right() {
        p.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], Stroke::new(1.0, color));
        x += step;
    }
}
