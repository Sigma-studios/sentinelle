//! The window. Reads snapshots from [`crate::core::Monitor`], sends it
//! commands, and otherwise knows nothing about how data is collected.

mod apps;
mod chart;
mod compute;
mod network;
mod storage;
mod theme;
mod widgets;

use crate::core::{Command, Config, Monitor};
use eframe::egui::{self, vec2, Align, Align2, Layout, RichText};
use std::time::{Duration, Instant};
use theme::*;

pub fn run(config: Config, demo: bool) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sentinelle")
            .with_app_id("sentinelle")
            .with_inner_size([1280.0, 1000.0])
            .with_min_inner_size([760.0, 620.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "sentinelle",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let monitor = if demo {
                Monitor::demo()
            } else {
                let ctx = cc.egui_ctx.clone();
                Monitor::start(config, move || ctx.request_repaint())
            };
            Ok(Box::new(App::new(monitor)))
        }),
    )
}

struct App {
    monitor: Monitor,
    storage: storage::StorageState,
    network: network::NetworkState,
    apps: apps::AppsState,
    notices: Vec<(String, Instant)>,
    screenshot: Option<(String, u32)>,
}

impl App {
    fn new(monitor: Monitor) -> Self {
        Self {
            monitor,
            storage: Default::default(),
            network: Default::default(),
            apps: Default::default(),
            notices: Vec::new(),
            screenshot: std::env::var("SENTINELLE_SCREENSHOT").ok().map(|p| (p, 0)),
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.monitor.snapshot();
        let now = Instant::now();
        for n in self.monitor.take_notices() {
            self.notices.push((n, now));
        }
        self.notices.retain(|(_, t)| now.duration_since(*t) < Duration::from_secs(5));

        let monitor = &self.monitor;
        let send = |c: Command| monitor.send(c);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(20, 16)))
            .show(ui, |ui| {
                let viewport_h = ui.available_height();
                let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if let Ok(y) = std::env::var("SENTINELLE_SCROLL") {
                    scroll = scroll.vertical_scroll_offset(y.parse().unwrap_or(0.0));
                }
                scroll.show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let top = ui.cursor().top();
                    header(ui, &snap);
                    ui.add_space(4.0);
                    section_title(ui, "Compute");
                    compute::show(ui, &snap);
                    storage::show(ui, &snap.disks, &mut self.storage, &send);
                    ui.add_space(GAP - 8.0);
                    network::show(ui, &snap.net, snap.interval, &mut self.network, &send);
                    ui.add_space(GAP - 8.0);
                    let used = ui.cursor().top() - top;
                    // Fill the rest of the window; scroll the page if it's small.
                    let apps_h = (viewport_h - used - 70.0).max(300.0);
                    apps::show(ui, &snap.apps, snap.memory.total, apps_h, &mut self.apps, &send);
                });
            });

        self.show_notices(ui.ctx());
        self.handle_screenshot(ui.ctx());
    }
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    widgets::section(ui, title, |_| {});
}

fn header(ui: &mut egui::Ui, snap: &crate::core::Snapshot) {
    ui.horizontal(|ui| {
        // Wordmark: a small load-blue mark + name.
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), egui::Sense::hover());
        let p = ui.painter();
        let c = r.center();
        for (i, h) in [6.0, 12.0, 9.0].iter().enumerate() {
            let x = c.x - 4.5 + i as f32 * 4.5;
            p.line_segment([egui::pos2(x, c.y + 6.0), egui::pos2(x, c.y + 6.0 - h)], egui::Stroke::new(2.5, LOAD));
        }
        ui.label(RichText::new("sentinelle").font(strong(16.0)).color(TEXT));
        ui.add_space(8.0);
        let mut parts = Vec::new();
        if !snap.host.hostname.is_empty() {
            parts.push(snap.host.hostname.clone());
        }
        if !snap.host.kernel.is_empty() {
            parts.push(format!("Linux {}", snap.host.kernel));
        }
        if snap.host.uptime_secs > 0 {
            parts.push(format!("up {}", duration(snap.host.uptime_secs)));
        }
        ui.label(widgets::muted(parts.join("  ·  ")));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let n: usize = snap.apps.iter().map(|a| a.procs.len()).sum();
            ui.label(widgets::muted(format!("{n} processes")));
        });
    });
}

impl App {
    fn show_notices(&self, ctx: &egui::Context) {
        if self.notices.is_empty() {
            return;
        }
        egui::Area::new("notices".into())
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
            .show(ctx, |ui| {
                for (msg, _) in &self.notices {
                    egui::Frame::new()
                        .fill(RAISED)
                        .stroke(egui::Stroke::new(1.0, BORDER))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(8.0, 12.0), egui::Sense::hover());
                                ui.painter().circle_filled(r.center(), 3.5, SERIOUS);
                                ui.label(RichText::new(msg).font(font(13.0)).color(TEXT));
                            });
                        });
                }
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Dev aid: `SENTINELLE_SCREENSHOT=out.ppm` saves a frame and exits.
    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, state)) = &mut self.screenshot else { return };
        // Give the collectors a few samples first.
        let delay: f64 = std::env::var("SENTINELLE_SCREENSHOT_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
        if *state == 0 && ctx.input(|i| i.time) >= delay {
            *state = 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let mut out = format!("P6\n{} {}\n255\n", img.size[0], img.size[1]).into_bytes();
            for px in &img.pixels {
                out.extend_from_slice(&[px.r(), px.g(), px.b()]);
            }
            let _ = std::fs::write(&*path, out);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
