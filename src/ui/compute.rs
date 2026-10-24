//! CPU, GPU and memory cards. Every card has the same anatomy so a row of
//! them lines up: title · hero figure · load chart · secondary chart · footer.

use super::chart::{self, core_strip, meter, Chart};
use super::theme::*;
use super::widgets::*;
use crate::core::{AppGroup, CpuStats, GpuStats, GpuVendor, MemStats, Snapshot};
use eframe::egui::{self, pos2, vec2, Align, CornerRadius, Layout, Rect, RichText, Sense, Ui};

const HERO_H: f32 = 44.0;
const CHART_H: f32 = 96.0;
const SECOND_H: f32 = 60.0;
const FOOTER_H: f32 = 30.0;
const MIN_CARD_W: f32 = 300.0;

enum Card<'a> {
    Cpu(&'a CpuStats),
    Gpu(&'a GpuStats),
    Memory(&'a MemStats, &'a [AppGroup]),
}

pub fn show(ui: &mut Ui, snap: &Snapshot) {
    let mut cards = vec![Card::Cpu(&snap.cpu)];
    cards.extend(snap.gpus.iter().map(Card::Gpu));
    cards.push(Card::Memory(&snap.memory, &snap.apps));

    let width = ui.available_width();
    let cols = (((width + GAP) / (MIN_CARD_W + GAP)).floor() as usize).clamp(1, cards.len());
    let card_w = ((width - GAP * (cols - 1) as f32) / cols as f32).floor();
    for row in cards.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for c in row {
                ui.allocate_ui_with_layout(vec2(card_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    card(ui, card_w, |ui| match c {
                        Card::Cpu(cpu) => cpu_card(ui, cpu, snap.interval),
                        Card::Gpu(gpu) => gpu_card(ui, gpu, snap.interval),
                        Card::Memory(mem, apps) => memory_card(ui, mem, apps, snap.interval),
                    });
                });
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
}

fn pct(v: f32) -> String {
    format!("{v:.0}%")
}

fn deg(v: f32) -> String {
    format!("{v:.0} °C")
}

/// Right-aligned stack of secondary facts next to the hero figure.
fn facts(ui: &mut Ui, lines: &[String]) {
    ui.with_layout(Layout::top_down(Align::Max), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for l in lines {
            ui.label(secondary(l.as_str()));
        }
    });
}

fn hero_row(ui: &mut Ui, value: &str, unit: &str, lines: &[String]) {
    fixed(ui, HERO_H, |ui| {
        ui.horizontal(|ui| {
            ui.label(hero(value, unit));
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| facts(ui, lines));
        });
    });
}

fn load_chart(ui: &mut Ui, data: &crate::core::History, color: egui::Color32, interval: f32) {
    let w = ui.available_width();
    Chart::new(data, color, &pct).grid(&[100.0, 50.0]).interval(interval).show(ui, vec2(w, CHART_H));
}

/// Temperature: its own small chart (never a second axis on the load chart).
fn temp_block(ui: &mut Ui, temp: Option<&crate::core::History>, interval: f32) {
    fixed(ui, SECOND_H, |ui| {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            key(ui, HEAT, "Temperature");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match temp.and_then(|t| t.last()) {
                Some(t) => value_with_status(ui, &deg(t), heat_status(t)),
                None => {
                    ui.label(muted("no sensor"));
                }
            });
        });
        if let Some(t) = temp {
            let (lo, hi) = chart::temp_range(t);
            let w = ui.available_width();
            Chart::new(t, HEAT, &deg).range(lo, hi).line_only().interval(interval).show(ui, vec2(w, 26.0));
        }
    });
}

fn cpu_card(ui: &mut Ui, cpu: &CpuStats, interval: f32) {
    card_title(ui, "CPU", &cpu.name);
    let usage = cpu.usage.last().unwrap_or(0.0);
    let threads = cpu.per_core.len();
    let mut lines = Vec::new();
    if let Some(f) = cpu.freq_mhz {
        lines.push(format!("{:.2} GHz", f / 1000.0));
    }
    lines.push(format!("{} cores · {} threads", cpu.physical_cores, threads));
    hero_row(ui, &format!("{usage:.0}"), "%", &lines);
    load_chart(ui, &cpu.usage, LOAD, interval);
    temp_block(ui, cpu.temp.as_ref(), interval);
    fixed(ui, FOOTER_H, |ui| {
        ui.add_space(4.0);
        let w = ui.available_width();
        core_strip(ui, vec2(w, FOOTER_H - 4.0), &cpu.per_core);
    });
}

fn gpu_card(ui: &mut Ui, gpu: &GpuStats, interval: f32) {
    let title = match gpu.vendor {
        GpuVendor::Nvidia => "GPU · NVIDIA",
        GpuVendor::Amd => "GPU · AMD",
        GpuVendor::Intel => "GPU · Intel",
    };
    card_title(ui, title, &gpu.name);
    let usage = gpu.usage.last().unwrap_or(0.0);
    let mut lines = Vec::new();
    if let Some(c) = gpu.clock_mhz {
        lines.push(format!("{c} MHz"));
    }
    if let Some(p) = gpu.power_w {
        lines.push(if p >= 10.0 { format!("{p:.0} W") } else { format!("{p:.1} W") });
    }
    if gpu.usage_is_clock {
        lines.push("clock vs. max".into());
    }
    hero_row(ui, &format!("{usage:.0}"), "%", &lines);
    load_chart(ui, &gpu.usage, LOAD, interval);
    temp_block(ui, gpu.temp.as_ref(), interval);
    fixed(ui, FOOTER_H, |ui| match (gpu.vram_used, gpu.vram_total) {
        (Some(u), Some(t)) if t > 0 => {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                key(ui, MEMORY, "VRAM");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(secondary(format!("{} / {}", bytes(u as f64), bytes(t as f64))));
                });
            });
            let w = ui.available_width();
            meter(ui, w, u as f32 / t as f32, MEMORY);
        }
        _ => {
            ui.add_space(6.0);
            ui.label(muted("No memory counters for this GPU"));
        }
    });
}

fn memory_card(ui: &mut Ui, mem: &MemStats, apps: &[AppGroup], interval: f32) {
    card_title(ui, "Memory", &format!("{} installed", bytes(mem.total as f64)));
    let used = bytes(mem.used as f64);
    let (num, unit) = used.split_once(' ').unwrap_or((&used, ""));
    let frac = if mem.total > 0 { mem.used as f32 / mem.total as f32 } else { 0.0 };
    let lines = vec![
        format!("{:.0}% in use", frac * 100.0),
        format!("{} available", bytes(mem.total.saturating_sub(mem.used) as f64)),
    ];
    hero_row(ui, num, &format!(" {unit}"), &lines);
    load_chart(ui, &mem.usage, MEMORY, interval);
    fixed(ui, SECOND_H, |ui| {
        ui.add_space(8.0);
        let segments = memory_segments(mem, apps);
        ui.horizontal(|ui| {
            key(ui, MEMORY, "By application");
            if let Some((MemSeg::App(_, g), size)) = segments.first() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(secondary(bytes(*size as f64)));
                    ui.add(egui::Label::new(muted(g.name.as_str())).truncate());
                });
            }
        });
        ui.add_space(4.0);
        let w = ui.available_width();
        memory_bar(ui, w, mem, &segments);
    });
    // Cache and swap side by side: two small meters.
    fixed(ui, FOOTER_H, |ui| {
        ui.add_space(2.0);
        let half = ((ui.available_width() - GAP) / 2.0).floor();
        let cache_frac = if mem.total > 0 { mem.cached as f32 / mem.total as f32 } else { 0.0 };
        // Half a card is too narrow for "used / total": the total is on hover.
        let (swap_text, swap_frac, swap_tip) = if mem.swap_total == 0 {
            (muted("none"), 0.0, "No swap configured".to_string())
        } else {
            (
                secondary(bytes(mem.swap_used as f64)),
                mem.swap_used as f32 / mem.swap_total as f32,
                format!("{} of {} swap in use", bytes(mem.swap_used as f64), bytes(mem.swap_total as f64)),
            )
        };
        let cache_tip = "Files kept in memory to be read faster; given back when apps need it".to_string();
        let halves = [
            ("Cache", secondary(bytes(mem.cached as f64)), cache_frac, MEMORY.gamma_multiply(0.55), cache_tip),
            ("Swap", swap_text, swap_frac, MEMORY, swap_tip),
        ];
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOTER_H - 2.0), Sense::hover());
        for (i, (label, text, frac, color, tip)) in halves.into_iter().enumerate() {
            let left = row.left() + i as f32 * (half + GAP);
            let rect = Rect::from_min_size(pos2(left, row.top()), vec2(half, row.height()));
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
            ui.horizontal(|ui| {
                key(ui, color, label);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(egui::Label::new(text).truncate()));
            });
            meter(&mut ui, half, frac, color);
            ui.interact(rect, ui.id().with(label), Sense::hover()).on_hover_text(tip);
        }
    });
}

/// Apps shown by name in the memory bar; the rest are folded together.
const MEM_TOP: usize = 5;

enum MemSeg<'a> {
    /// Rank (largest first) and the app.
    App(usize, &'a AppGroup),
    /// The other apps, and how many.
    OtherApps(usize),
    /// Used memory no app accounts for: the kernel, shared memory, caches
    /// that can't be dropped.
    System,
}

/// Used memory split into the apps holding the most, the other apps, and the
/// rest. App sizes are each app's own resident memory (shared pages left
/// out), as in the Applications list, so nothing is counted twice.
fn memory_segments<'a>(mem: &MemStats, apps: &'a [AppGroup]) -> Vec<(MemSeg<'a>, u64)> {
    let mut by_size: Vec<&AppGroup> = apps.iter().filter(|g| g.mem > 0).collect();
    by_size.sort_by(|a, b| b.mem.cmp(&a.mem).then_with(|| a.name.cmp(&b.name)));
    let rest = by_size.split_off(by_size.len().min(MEM_TOP));
    let mut out: Vec<(MemSeg, u64)> = by_size.iter().enumerate().map(|(i, g)| (MemSeg::App(i, g), g.mem)).collect();
    let rest_size: u64 = rest.iter().map(|g| g.mem).sum();
    if rest_size > 0 {
        out.push((MemSeg::OtherApps(rest.len()), rest_size));
    }
    let apps_size: u64 = out.iter().map(|(_, s)| s).sum();
    let system = mem.used.saturating_sub(apps_size);
    if system > 0 {
        out.push((MemSeg::System, system));
    }
    out
}

/// A stacked bar over all installed memory, like a Storage bar: one segment
/// per entry, free memory left as track. Hover names a segment.
fn memory_bar(ui: &mut Ui, width: f32, mem: &MemStats, segments: &[(MemSeg, u64)]) {
    let (bar, resp) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::same(4), TRACK);
    let total = mem.total.max(1) as f32;
    let gap = 2.0;
    let mut x = bar.left();
    let rects: Vec<Rect> = segments
        .iter()
        .map(|(_, size)| {
            let sw = bar.width() * *size as f32 / total;
            let r = Rect::from_min_max(pos2(x, bar.top()), pos2((x + sw - gap).max(x + 1.0).min(bar.right()), bar.bottom()));
            x += sw;
            r
        })
        .collect();
    let hovered = resp.hover_pos().and_then(|pp| rects.iter().position(|r| r.expand2(vec2(gap / 2.0, 0.0)).contains(pp)));
    for (i, ((seg, _), r)) in segments.iter().zip(&rects).enumerate() {
        let mut c = match seg {
            MemSeg::App(rank, _) => MEM_RAMP[(*rank).min(MEM_RAMP.len() - 1)],
            MemSeg::OtherApps(_) => OTHER,
            MemSeg::System => FAINT,
        };
        if hovered.is_some_and(|h| h != i) {
            c = c.gamma_multiply(0.35);
        }
        let cr = if i == 0 { CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 } } else { CornerRadius::ZERO };
        if let MemSeg::System = seg {
            hatch(p, *r, c);
            continue;
        }
        p.rect_filled(*r, cr, c);
        // The name inside the segment when it fits: dark on the bright
        // shades, light on the deep ones.
        if let MemSeg::App(rank, g) = seg {
            let ink = if *rank < 2 { BG } else { TEXT };
            let galley = ui.fonts_mut(|f| f.layout_no_wrap(g.name.clone(), font(11.0), ink));
            if galley.size().x + 10.0 <= r.width() {
                p.galley(pos2(r.left() + 5.0, r.center().y - galley.size().y / 2.0), galley, ink);
            }
        }
    }
    if let Some(i) = hovered {
        let (seg, size) = &segments[i];
        let pct = *size as f32 / total * 100.0;
        resp.on_hover_ui_at_pointer(|ui| {
            let (title, note) = match seg {
                MemSeg::App(_, g) => (
                    g.name.clone(),
                    if g.procs.len() == 1 { "1 process".to_string() } else { format!("{} processes", g.procs.len()) },
                ),
                MemSeg::OtherApps(n) => (format!("{n} more apps"), "Smaller apps and services together".to_string()),
                MemSeg::System => ("System".to_string(), "Kernel, shared memory and other memory no app accounts for".to_string()),
            };
            ui.label(RichText::new(title).font(strong(13.0)).color(TEXT));
            ui.label(secondary(format!("{} · {pct:.0}% of memory", bytes(*size as f64))));
            ui.label(muted(note));
        });
    }
}
