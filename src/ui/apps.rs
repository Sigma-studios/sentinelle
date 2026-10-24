//! Application list: games, apps, background services and system
//! processes, each expandable into its process tree.

use super::theme::*;
use super::widgets::*;
use crate::core::{AppGroup, AppKind, Command, ProcInfo, Signal};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Rect, Sense, Stroke, Ui};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const ROW_H: f32 = 30.0;
const CONFIRM_FOR: Duration = Duration::from_secs(3);
const SECTIONS: [(AppKind, &str); 4] = [
    (AppKind::Game, "Games"),
    (AppKind::App, "Applications"),
    (AppKind::Background, "Background"),
    (AppKind::System, "System"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Name,
    Cpu,
    Memory,
    Disk,
}

pub struct AppsState {
    expanded: HashSet<String>,
    closed_sections: HashSet<u8>,
    filter: String,
    /// Column and whether it's reversed from its natural direction (largest
    /// first, or A to Z for names). None leaves rows where they are.
    sort: Option<(Col, bool)>,
    /// Order shown last frame; reused while the pointer is over the list so
    /// rows don't jump away from the cursor, and kept as is when unsorted.
    order: HashMap<String, usize>,
    confirm: Option<(String, Instant)>,
}

impl Default for AppsState {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            closed_sections: [section_id(AppKind::Background), section_id(AppKind::System)].into(),
            filter: String::new(),
            sort: Some((Col::Cpu, false)),
            order: HashMap::new(),
            confirm: None,
        }
    }
}

fn section_id(k: AppKind) -> u8 {
    k as u8
}

enum Row<'a> {
    Section { kind: AppKind, title: &'static str, groups: Vec<&'a AppGroup> },
    App(&'a AppGroup),
    Proc(&'a AppGroup, &'a ProcInfo),
}

struct Columns {
    name: Rect,
    cpu: Rect,
    mem: Rect,
    disk: Rect,
    action: Rect,
}

fn columns(row: Rect) -> Columns {
    let action_w = 76.0;
    let disk_w = 96.0;
    let mem_w = 128.0;
    let cpu_w = 112.0;
    let mut x = row.right();
    let mut take = |w: f32| {
        let r = Rect::from_min_max(pos2(x - w, row.top()), pos2(x, row.bottom()));
        x -= w;
        r
    };
    let action = take(action_w);
    let disk = take(disk_w);
    let mem = take(mem_w);
    let cpu = take(cpu_w);
    let name = Rect::from_min_max(row.min, pos2(x - 8.0, row.bottom()));
    Columns { name, cpu, mem, disk, action }
}

pub fn show(ui: &mut Ui, apps: &[AppGroup], total_mem: u64, height: f32, state: &mut AppsState, send: &dyn Fn(Command)) {
    section(ui, "Applications", |ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text("Filter by name or command")
                .desired_width(220.0)
                .font(font(12.0)),
        );
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.filter.clear();
        }
    });
    let width = ui.available_width();
    card(ui, width, |ui| {
        ui.set_height(height.max(200.0));
        // A header click has to show at once, though the pointer is still
        // over the list.
        let resorted = header(ui, state);
        let rows = build_rows(apps, state, ui.ui_contains_pointer() && !resorted);
        let now = Instant::now();
        if state.confirm.as_ref().is_some_and(|(_, t)| now.duration_since(*t) > CONFIRM_FOR) {
            state.confirm = None;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                match row {
                    Row::Section { kind, title, groups } => section_row(ui, *kind, title, groups, state),
                    Row::App(g) => app_row(ui, g, total_mem, state, send),
                    Row::Proc(g, p) => proc_row(ui, g, p, total_mem, state, send),
                }
            }
        });
        if state.confirm.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    });
}

fn header(ui: &mut Ui, state: &mut AppsState) -> bool {
    let mut clicked = false;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    let c = columns(rect);
    let cells = [
        (Col::Name, c.name, "Name", Align2::LEFT_CENTER),
        (Col::Cpu, c.cpu, "CPU", Align2::RIGHT_CENTER),
        (Col::Memory, c.mem, "Memory", Align2::RIGHT_CENTER),
        (Col::Disk, c.disk, "Disk", Align2::RIGHT_CENTER),
    ];
    for (col, r, label, align) in cells {
        let resp = ui.interact(r, ui.id().with(label), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            clicked = true;
            // Natural direction, then reversed, then unsorted.
            state.sort = match state.sort {
                Some((c, false)) if c == col => Some((col, true)),
                Some((c, true)) if c == col => None,
                _ => Some((col, false)),
            };
        }
        let reversed = match state.sort {
            Some((c, r)) if c == col => Some(r),
            _ => None,
        };
        let active = reversed.is_some();
        let color = if active || resp.hovered() { TEXT } else { MUTED };
        let x = if align == Align2::LEFT_CENTER { r.left() + 10.0 } else { r.right() - 8.0 };
        let tr = ui.painter().text(pos2(x, r.center().y), align, label, strong(11.5), color);
        if active {
            // Sort indicator: a small caret, down for the natural direction
            // and up once reversed.
            let c = if align == Align2::LEFT_CENTER {
                pos2(tr.right() + 8.0, r.center().y)
            } else {
                pos2(tr.left() - 8.0, r.center().y)
            };
            caret(ui.painter(), c, reversed == Some(true), TEXT);
        }
    }
    ui.painter().hline(rect.x_range(), rect.bottom() + 0.5, Stroke::new(1.0, BORDER));
    ui.add_space(4.0);
    clicked
}

fn matches(g: &AppGroup, f: &str) -> (bool, bool) {
    if f.is_empty() {
        return (true, false);
    }
    let in_name = g.name.to_lowercase().contains(f);
    let in_procs = g.procs.iter().any(|p| p.name.to_lowercase().contains(f) || p.cmdline.to_lowercase().contains(f) || p.pid.to_string() == f);
    (in_name || in_procs, in_procs && !in_name)
}

fn build_rows<'a>(apps: &'a [AppGroup], state: &mut AppsState, freeze: bool) -> Vec<Row<'a>> {
    let filter = state.filter.trim().to_lowercase();
    let mut groups: Vec<(&AppGroup, bool)> = apps
        .iter()
        .filter_map(|g| {
            let (show, expand) = matches(g, &filter);
            show.then_some((g, expand))
        })
        .collect();

    match state.sort {
        Some((col, reversed)) if !(freeze && !state.order.is_empty()) => {
            let key = |g: &AppGroup| -> f64 {
                match col {
                    Col::Name => 0.0,
                    Col::Cpu => -(g.cpu as f64),
                    Col::Memory => -(g.mem as f64),
                    Col::Disk => -g.io,
                }
            };
            groups.sort_by(|(a, _), (b, _)| {
                let by_value = key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal);
                let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                // Equal values stay A to Z, unless names are the sort itself.
                match (reversed, col) {
                    (false, _) => by_value.then(by_name),
                    (true, Col::Name) => by_name.reverse(),
                    (true, _) => by_value.reverse().then(by_name),
                }
            });
            state.order = groups.iter().enumerate().map(|(i, (g, _))| (g.key.clone(), i)).collect();
        }
        sort => {
            // Keep last frame's order; anything new goes at the end, A to Z
            // (the core hands groups over in no particular order).
            let by_last = |g: &AppGroup| state.order.get(&g.key).copied().unwrap_or(usize::MAX);
            groups.sort_by(|(a, _), (b, _)| by_last(a).cmp(&by_last(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            if sort.is_none() {
                // Remember where new groups landed, and forget ones that are
                // gone. Keys hidden by the filter keep their place.
                let live: HashSet<&str> = apps.iter().map(|g| g.key.as_str()).collect();
                state.order.retain(|k, _| live.contains(k.as_str()));
                let mut next = state.order.values().max().map_or(0, |m| m + 1);
                for (g, _) in &groups {
                    state.order.entry(g.key.clone()).or_insert_with(|| {
                        next += 1;
                        next - 1
                    });
                }
            }
        }
    }

    let mut rows = Vec::new();
    for (kind, title) in SECTIONS {
        let in_section: Vec<_> = groups.iter().filter(|(g, _)| g.kind == kind).collect();
        if in_section.is_empty() {
            continue;
        }
        rows.push(Row::Section { kind, title, groups: in_section.iter().map(|(g, _)| *g).collect() });
        let open = !state.closed_sections.contains(&section_id(kind)) || !filter.is_empty();
        if !open {
            continue;
        }
        for (g, auto_expand) in in_section {
            rows.push(Row::App(g));
            if (state.expanded.contains(&g.key) || *auto_expand) && g.procs.len() > 1 {
                rows.extend(g.procs.iter().map(|p| Row::Proc(g, p)));
            }
        }
    }
    rows
}

fn chevron(p: &egui::Painter, center: egui::Pos2, open: bool, color: Color32) {
    let s = 3.5;
    let pts = if open {
        vec![pos2(center.x - s, center.y - s / 2.0), pos2(center.x, center.y + s / 2.0), pos2(center.x + s, center.y - s / 2.0)]
    } else {
        vec![pos2(center.x - s / 2.0, center.y - s), pos2(center.x + s / 2.0, center.y), pos2(center.x - s / 2.0, center.y + s)]
    };
    p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
}

fn caret(p: &egui::Painter, center: egui::Pos2, up: bool, color: Color32) {
    let s = 3.5;
    let dy = if up { -s / 2.0 } else { s / 2.0 };
    let pts = vec![pos2(center.x - s, center.y - dy), pos2(center.x, center.y + dy), pos2(center.x + s, center.y - dy)];
    p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
}

/// Value text right-aligned in a cell, with a thin bar showing its share.
fn bar_cell(p: &egui::Painter, r: Rect, text: &str, frac: f32, color: Color32, text_color: Color32) {
    let track = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 2.0), vec2(36.0, 4.0));
    p.rect_filled(track, CornerRadius::same(2), TRACK);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.005 {
        let fill = Rect::from_min_size(track.min, vec2((track.width() * f).max(3.0), track.height()));
        p.rect_filled(fill, CornerRadius::same(2), color);
    }
    p.text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, text, font(12.5), text_color);
}

fn cpu_text(v: f32) -> String {
    if v < 0.05 {
        "0%".into()
    } else if v < 10.0 {
        format!("{v:.1}%")
    } else {
        format!("{v:.0}%")
    }
}

fn io_text(v: f64) -> String {
    if v < 1.0 {
        "—".into()
    } else {
        rate(v)
    }
}

fn section_row(ui: &mut Ui, kind: AppKind, title: &str, groups: &[&AppGroup], state: &mut AppsState) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
    let id = section_id(kind);
    let open = !state.closed_sections.contains(&id) || !state.filter.trim().is_empty();
    if resp.clicked() && !state.closed_sections.remove(&id) {
        state.closed_sections.insert(id);
    }
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect.shrink2(vec2(0.0, 2.0)), CornerRadius::same(6), SURFACE_HOVER);
    }
    chevron(p, pos2(rect.left() + 14.0, rect.center().y), open, MUTED);
    let galley = ui.fonts_mut(|f| f.layout_job(eyebrow(title, TEXT_2)));
    let tw = galley.size().x;
    p.galley(pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0), galley, TEXT_2);
    p.text(pos2(rect.left() + 36.0 + tw, rect.center().y), Align2::LEFT_CENTER, groups.len().to_string(), font(11.5), FAINT);

    let c = columns(rect);
    let cpu: f32 = groups.iter().map(|g| g.cpu).sum();
    let mem: u64 = groups.iter().map(|g| g.mem).sum();
    let io: f64 = groups.iter().map(|g| g.io).sum();
    p.text(pos2(c.cpu.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, cpu_text(cpu), font(11.5), MUTED);
    p.text(pos2(c.mem.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, bytes(mem as f64), font(11.5), MUTED);
    p.text(pos2(c.disk.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, io_text(io), font(11.5), MUTED);
    resp.on_hover_cursor(CursorIcon::PointingHand);
}

/// The End button: first click arms it, second click within a few seconds
/// sends SIGTERM. Returns true when the action should happen.
fn end_button(ui: &mut Ui, rect: Rect, id: &str, row_hovered: bool, state: &mut AppsState) -> bool {
    let armed = state.confirm.as_ref().is_some_and(|(k, _)| k == id);
    if !row_hovered && !armed {
        return false;
    }
    let r = Rect::from_center_size(pos2(rect.right() - 36.0, rect.center().y), vec2(60.0, 22.0));
    let resp = ui.interact(r, ui.id().with(("end", id)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    if armed {
        p.rect_filled(r, CornerRadius::same(6), CRITICAL);
        p.text(r.center(), Align2::CENTER_CENTER, "Confirm", strong(11.5), TEXT);
    } else {
        let stroke = if resp.hovered() { CRITICAL } else { BORDER };
        p.rect(r, CornerRadius::same(6), SURFACE, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, "End", font(12.0), if resp.hovered() { TEXT } else { TEXT_2 });
    }
    let resp = resp.on_hover_text(if armed { "Click again to end it" } else { "End (asks to confirm). Right-click for more." });
    if resp.clicked() {
        if armed {
            state.confirm = None;
            return true;
        }
        state.confirm = Some((id.to_string(), Instant::now()));
    }
    false
}

fn context_menu(resp: &egui::Response, pids: &[u32], cmdline: Option<&str>, send: &dyn Fn(Command)) {
    resp.context_menu(|ui| {
        ui.set_min_width(180.0);
        if ui.button("End").on_hover_text("Ask politely (SIGTERM)").clicked() {
            send(Command::Signal { pids: pids.to_vec(), signal: Signal::Term });
            ui.close();
        }
        if ui.button("Force kill").on_hover_text("SIGKILL, no chance to save").clicked() {
            send(Command::Signal { pids: pids.to_vec(), signal: Signal::Kill });
            ui.close();
        }
        ui.separator();
        let label = if pids.len() == 1 { "Copy PID" } else { "Copy PIDs" };
        if ui.button(label).clicked() {
            let s: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
            ui.ctx().copy_text(s.join(" "));
            ui.close();
        }
        if let Some(c) = cmdline {
            if ui.button("Copy command").clicked() {
                ui.ctx().copy_text(c.to_string());
                ui.close();
            }
        }
    });
}

fn app_row(ui: &mut Ui, g: &AppGroup, total_mem: u64, state: &mut AppsState, send: &dyn Fn(Command)) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    let expandable = g.procs.len() > 1;
    let open = state.expanded.contains(&g.key);
    let c = columns(rect);

    {
        let p = ui.painter();
        if hovered {
            p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), CornerRadius::same(6), SURFACE_HOVER);
        }
        if expandable {
            chevron(p, pos2(rect.left() + 14.0, rect.center().y), open, if hovered { TEXT_2 } else { FAINT });
        }
        let name_x = rect.left() + 28.0;
        let name = ui.fonts_mut(|f| {
            f.layout(g.name.clone(), font(13.0), TEXT, (c.name.right() - name_x - 40.0).max(20.0))
        });
        let nw = name.size().x;
        p.with_clip_rect(c.name).galley(pos2(name_x, rect.center().y - name.size().y / 2.0), name, TEXT);
        if expandable {
            p.text(pos2(name_x + nw + 8.0, rect.center().y), Align2::LEFT_CENTER, g.procs.len().to_string(), font(11.5), FAINT);
        }
        bar_cell(p, c.cpu, &cpu_text(g.cpu), g.cpu / 100.0, LOAD, TEXT_2);
        let mf = if total_mem > 0 { g.mem as f32 / total_mem as f32 } else { 0.0 };
        bar_cell(p, c.mem, &bytes(g.mem as f64), mf, MEMORY, TEXT_2);
        let io_color = if g.io < 1.0 { FAINT } else { TEXT_2 };
        p.text(pos2(c.disk.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, io_text(g.io), font(12.5), io_color);
    }

    let pids: Vec<u32> = g.procs.iter().map(|p| p.pid).collect();
    if end_button(ui, c.action, &g.key, hovered, state) {
        send(Command::Signal { pids: pids.clone(), signal: Signal::Term });
    }
    let on_button = ui.ctx().pointer_hover_pos().is_some_and(|pp| pp.x > c.action.left());
    if resp.clicked() && expandable && !on_button && !state.expanded.remove(&g.key) {
        state.expanded.insert(g.key.clone());
    }
    let resp = if expandable { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    context_menu(&resp, &pids, g.procs.first().map(|p| &*p.cmdline), send);
}

fn proc_row(ui: &mut Ui, g: &AppGroup, pr: &ProcInfo, total_mem: u64, state: &mut AppsState, send: &dyn Fn(Command)) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    let c = columns(rect);
    {
        let p = ui.painter();
        if hovered {
            p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), CornerRadius::same(6), SURFACE_HOVER);
        }
        // Tree guide
        let indent = 28.0 + 14.0 * (pr.depth as f32 + 1.0);
        let gx = rect.left() + 14.0;
        p.vline(gx, rect.y_range(), Stroke::new(1.0, BORDER));
        let name_x = rect.left() + indent;
        let name = ui.fonts_mut(|f| f.layout(pr.name.clone(), font(12.5), TEXT_2, (c.name.right() - name_x - 60.0).max(20.0)));
        let nw = name.size().x;
        let clip = p.with_clip_rect(c.name);
        clip.galley(pos2(name_x, rect.center().y - name.size().y / 2.0), name, TEXT_2);
        clip.text(pos2(name_x + nw + 8.0, rect.center().y), Align2::LEFT_CENTER, pr.pid.to_string(), mono(11.0), FAINT);
        bar_cell(p, c.cpu, &cpu_text(pr.cpu), pr.cpu / 100.0, LOAD, MUTED);
        let mf = if total_mem > 0 { pr.mem as f32 / total_mem as f32 } else { 0.0 };
        bar_cell(p, c.mem, &bytes(pr.mem as f64), mf, MEMORY, MUTED);
        p.text(pos2(c.disk.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, io_text(pr.io), font(12.0), FAINT);
    }
    let id = format!("{}#{}", g.key, pr.pid);
    if end_button(ui, c.action, &id, hovered, state) {
        send(Command::Signal { pids: vec![pr.pid], signal: Signal::Term });
    }
    let resp = resp.on_hover_text_at_pointer(&*pr.cmdline);
    context_menu(&resp, &[pr.pid], Some(&pr.cmdline), send);
}
