//! CPU, GPU and memory cards. Every card has the same anatomy so a row of
//! them lines up: title · hero figure · load chart · secondary chart · footer.

use super::chart::{self, core_strip, meter, Chart};
use super::theme::*;
use super::widgets::*;
use crate::core::{CpuStats, GpuStats, GpuVendor, MemStats, Snapshot};
use eframe::egui::{self, vec2, Align, Layout, Ui};

const HERO_H: f32 = 44.0;
const CHART_H: f32 = 96.0;
const SECOND_H: f32 = 60.0;
const FOOTER_H: f32 = 30.0;
const MIN_CARD_W: f32 = 300.0;

enum Card<'a> {
    Cpu(&'a CpuStats),
    Gpu(&'a GpuStats),
    Memory(&'a MemStats),
}

pub fn show(ui: &mut Ui, snap: &Snapshot) {
    let mut cards = vec![Card::Cpu(&snap.cpu)];
    cards.extend(snap.gpus.iter().map(Card::Gpu));
    cards.push(Card::Memory(&snap.memory));

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
                        Card::Memory(mem) => memory_card(ui, mem, snap.interval),
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

fn memory_card(ui: &mut Ui, mem: &MemStats, interval: f32) {
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
        ui.horizontal(|ui| {
            key(ui, MEMORY, "Cache & buffers");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(secondary(bytes(mem.cached as f64)));
            });
        });
        let w = ui.available_width();
        meter(ui, w, if mem.total > 0 { mem.cached as f32 / mem.total as f32 } else { 0.0 }, MEMORY.gamma_multiply(0.55));
    });
    fixed(ui, FOOTER_H, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            key(ui, MEMORY, "Swap");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if mem.swap_total == 0 {
                    ui.label(muted("none"));
                } else {
                    ui.label(secondary(format!("{} / {}", bytes(mem.swap_used as f64), bytes(mem.swap_total as f64))));
                }
            });
        });
        let w = ui.available_width();
        let f = if mem.swap_total > 0 { mem.swap_used as f32 / mem.swap_total as f32 } else { 0.0 };
        meter(ui, w, f, MEMORY);
    });
}
