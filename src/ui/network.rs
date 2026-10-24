//! Connection, throughput and latency in one compact card.

use super::chart::{nice_ceil, Chart};
use super::theme::*;
use super::widgets::*;
use crate::core::{Command, History, LinkKind, NetStats};
use eframe::egui::{self, vec2, Align, Color32, Key, Layout, RichText, Ui};

#[derive(Default)]
pub struct NetworkState {
    editing_host: Option<String>,
}

const ROW_H: f32 = 108.0;

pub fn show(ui: &mut Ui, net: &NetStats, interval: f32, state: &mut NetworkState, send: &dyn Fn(Command)) {
    section(ui, "Network", |_| {});
    let width = ui.available_width();
    card(ui, width, |ui| {
        let inner = ui.available_width();
        let col_gap = 28.0;
        let conn_w = (inner * 0.26).clamp(200.0, 300.0);
        let rest = inner - conn_w - col_gap * 2.0;
        let traffic_w = (rest * 0.55).floor();
        let ping_w = rest - traffic_w;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = col_gap;
            column(ui, conn_w, |ui| connection(ui, net));
            column(ui, traffic_w, |ui| traffic(ui, net, interval));
            column(ui, ping_w, |ui| latency(ui, net, interval, state, send));
        });
    });
}

fn column(ui: &mut Ui, w: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(w, ROW_H), Layout::top_down(Align::Min), |ui| {
        ui.set_width(w);
        ui.set_height(ROW_H);
        ui.spacing_mut().item_spacing.x = 8.0;
        add(ui);
    });
}

fn connection(ui: &mut Ui, net: &NetStats) {
    ui.label(eyebrow("Connection", MUTED));
    ui.add_space(6.0);
    if net.iface.is_empty() {
        ui.label(RichText::new("Offline").font(strong(18.0)).color(TEXT));
        ui.label(muted("No default route"));
        return;
    }
    ui.horizontal(|ui| {
        match net.kind {
            LinkKind::Wireless => wifi_icon(ui, net.signal),
            _ => wired_icon(ui),
        }
        let name = net.name.clone().unwrap_or_else(|| match net.kind {
            LinkKind::Wireless => "Wi-Fi".into(),
            LinkKind::Wired => "Ethernet".into(),
            _ => net.iface.clone(),
        });
        ui.add(egui::Label::new(RichText::new(name).font(strong(18.0)).color(TEXT)).truncate());
    });
    ui.add_space(4.0);
    let mut line = net.iface.clone();
    if let Some(ip) = &net.ipv4 {
        line.push_str(&format!(" · {ip}"));
    }
    ui.label(secondary(line));
    let mut details = Vec::new();
    if let Some(m) = net.link_mbps {
        details.push(if m >= 1000.0 { format!("{:.1} Gb/s link", m / 1000.0) } else { format!("{m:.0} Mb/s link") });
    }
    if let Some(s) = net.signal {
        details.push(format!("signal {s}%"));
    }
    if !details.is_empty() {
        ui.label(muted(details.join(" · ")));
    }
}

fn traffic(ui: &mut Ui, net: &NetStats, interval: f32) {
    ui.label(eyebrow("Traffic", MUTED));
    ui.add_space(4.0);
    // Two small multiples, each with its own labeled scale.
    for (label, h, color) in [("Download", &net.rx, DOWN), ("Upload", &net.tx, UP)] {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.allocate_ui_with_layout(vec2(118.0, 36.0), Layout::top_down(Align::Min), |ui| {
                ui.set_min_width(118.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                key(ui, color, label);
                ui.label(RichText::new(rate(h.last().unwrap_or(0.0) as f64)).font(strong(14.0)).color(TEXT));
            });
            let top = nice_ceil(h.max().unwrap_or(0.0).max(1024.0));
            let fmt = |v: f32| rate(v as f64);
            let w = ui.available_width();
            Chart::new(h, color, &fmt).range(0.0, top).grid(&[top]).sparkline().interval(interval).show(ui, vec2(w, 36.0));
        });
        ui.add_space(4.0);
    }
}

struct PingSummary {
    last: Option<f32>,
    loss: f32,
    jitter: Option<f32>,
}

fn summarize(h: &History) -> PingSummary {
    let n = h.len().max(1) as f32;
    let lost = h.iter().filter(|v| v.is_nan()).count() as f32;
    let vals: Vec<f32> = h.iter().rev().take(30).filter(|v| v.is_finite()).collect();
    let jitter = (vals.len() >= 3)
        .then(|| vals.windows(2).map(|w| (w[0] - w[1]).abs()).sum::<f32>() / (vals.len() - 1) as f32);
    PingSummary { last: h.last(), loss: lost / n * 100.0, jitter }
}

fn ping_status(h: &History) -> (Color32, &'static str) {
    let recent: Vec<f32> = h.iter().rev().take(3).collect();
    if !recent.is_empty() && recent.iter().all(|v| v.is_nan()) {
        return (CRITICAL, "Unreachable");
    }
    let v = h.iter().rev().find(|v| v.is_finite()).unwrap_or(0.0);
    match v {
        v if v < 40.0 => (GOOD, "Good"),
        v if v < 100.0 => (WARNING, "Fair"),
        _ => (SERIOUS, "Slow"),
    }
}

fn latency(ui: &mut Ui, net: &NetStats, interval: f32, state: &mut NetworkState, send: &dyn Fn(Command)) {
    let s = summarize(&net.ping);
    ui.horizontal(|ui| {
        ui.label(eyebrow("Latency", MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(buf) = &mut state.editing_host {
                let resp = ui.add(egui::TextEdit::singleline(buf).desired_width(150.0).font(font(12.0)));
                if resp.lost_focus() {
                    if ui.input(|i| i.key_pressed(Key::Enter)) && !buf.trim().is_empty() {
                        send(Command::SetPingHost(buf.trim().to_string()));
                    }
                    state.editing_host = None;
                } else {
                    resp.request_focus();
                }
            } else if ghost_button(ui, &net.ping_host).on_hover_text("Change the server to ping").clicked() {
                state.editing_host = Some(net.ping_host.clone());
            }
        });
    });
    ui.horizontal(|ui| {
        let text = match s.last {
            Some(v) if v.is_finite() => format!("{v:.0}"),
            _ => "—".into(),
        };
        ui.label(hero(&text, " ms"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !net.ping.is_empty() {
                let (c, label) = ping_status(&net.ping);
                status_badge(ui, c, label);
            }
        });
    });
    let top = nice_ceil(net.ping.max().unwrap_or(10.0).max(10.0) * 1.1);
    let fmt = |v: f32| format!("{v:.0} ms");
    let w = ui.available_width();
    Chart::new(&net.ping, NEUTRAL, &fmt).range(0.0, top).grid(&[top]).sparkline().line_only().interval(interval).show(ui, vec2(w, 30.0));
    let mut facts = vec![net.ping_method.to_string()];
    if let Some(j) = s.jitter {
        facts.push(format!("±{j:.1} ms jitter"));
    }
    facts.push(format!("{:.0}% loss", s.loss));
    ui.label(muted(facts.join(" · ")));
}
