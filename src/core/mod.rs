//! Data collection. Nothing in here knows about the UI: the UI holds a
//! [`Monitor`], reads immutable [`Snapshot`]s from it and sends [`Command`]s.

mod apps;
mod cpu;
pub mod demo;
mod disk;
mod gpu;
mod history;
mod hwmon;
mod mem;
mod net;
mod nvml;
mod ping;
mod procs;
mod sampler;
mod steam;
mod util;

pub use history::History;

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// Everything the UI needs to draw one frame. Cheap to share, never mutated.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub host: HostInfo,
    pub cpu: CpuStats,
    pub memory: MemStats,
    pub gpus: Vec<GpuStats>,
    pub disks: Vec<DiskStats>,
    pub net: NetStats,
    pub apps: Vec<AppGroup>,
    /// Seconds between samples; also the x-step of every history.
    pub interval: f32,
}

#[derive(Clone, Default)]
pub struct HostInfo {
    pub hostname: String,
    pub kernel: String,
    pub uptime_secs: u64,
}

#[derive(Clone, Default)]
pub struct CpuStats {
    pub name: String,
    /// Total usage, 0..=100.
    pub usage: History,
    /// Package temperature in °C, if a sensor was found.
    pub temp: Option<History>,
    pub freq_mhz: Option<f32>,
    /// Per logical core usage, 0..=100.
    pub per_core: Vec<f32>,
    pub physical_cores: usize,
}

#[derive(Clone, Default)]
pub struct MemStats {
    pub total: u64,
    pub used: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// Used RAM as a percentage, 0..=100.
    pub usage: History,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GpuVendor {
    Amd,
    Nvidia,
    Intel,
}

#[derive(Clone)]
pub struct GpuStats {
    pub name: String,
    pub vendor: GpuVendor,
    /// Busy percentage. For GPUs whose driver doesn't expose a busy counter
    /// (Intel), this is the current clock as a percentage of max clock and
    /// `usage_is_clock` is set.
    pub usage: History,
    pub usage_is_clock: bool,
    pub temp: Option<History>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub power_w: Option<f32>,
    pub clock_mhz: Option<u32>,
}

#[derive(Clone)]
pub struct DiskStats {
    /// Stable identifier (the backing device).
    pub id: String,
    pub mount_points: Vec<PathBuf>,
    pub device: String,
    pub fs_type: String,
    pub total: u64,
    pub used: u64,
    /// Where the used space is, largest first. Empty until the first scan.
    pub segments: Vec<DiskSegment>,
    /// `Some(bytes_seen_so_far)` while a scan is running.
    pub scanning: Option<u64>,
    /// Seconds since the segments were computed.
    pub scanned_ago: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SegmentKind {
    /// A directory and everything below it.
    Dir,
    /// The files of a directory that aren't covered by other segments.
    Rest,
    /// Many small leftovers folded together.
    Other,
    /// Used space the scanner couldn't see (permissions, fs metadata).
    Hidden,
}

#[derive(Clone, Debug)]
pub struct DiskSegment {
    pub path: PathBuf,
    pub size: u64,
    pub kind: SegmentKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum LinkKind {
    #[default]
    None,
    Wired,
    Wireless,
    Other,
}

#[derive(Clone, Default)]
pub struct NetStats {
    pub iface: String,
    pub kind: LinkKind,
    /// SSID or connection name.
    pub name: Option<String>,
    pub ipv4: Option<String>,
    pub link_mbps: Option<f32>,
    /// Wireless signal, 0..=100.
    pub signal: Option<u8>,
    /// Bytes per second.
    pub rx: History,
    pub tx: History,
    pub ping_host: String,
    /// Round trip in ms; NaN marks a lost probe.
    pub ping: History,
    pub ping_method: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppKind {
    Game,
    App,
    Background,
    System,
}

#[derive(Clone)]
pub struct AppGroup {
    /// Stable across samples; use it to remember UI state.
    pub key: String,
    pub name: String,
    pub kind: AppKind,
    /// Percentage of total CPU capacity, 0..=100.
    pub cpu: f32,
    pub mem: u64,
    /// Disk read + write, bytes per second.
    pub io: f64,
    pub procs: Vec<ProcInfo>,
}

#[derive(Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cmdline: Arc<str>,
    /// Depth below the group's root process, for indentation.
    pub depth: u8,
    pub cpu: f32,
    pub mem: u64,
    pub io: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signal {
    Term,
    Kill,
}

pub enum Command {
    Signal { pids: Vec<u32>, signal: Signal },
    RescanDisks,
    SetPingHost(String),
}

#[derive(Clone)]
pub struct Config {
    pub interval: Duration,
    pub ping_host: String,
    pub history_len: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(1000),
            ping_host: "1.1.1.1".into(),
            history_len: 90,
        }
    }
}

pub(crate) struct Shared {
    snapshot: RwLock<Arc<Snapshot>>,
    notices: Mutex<Vec<String>>,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn publish(&self, snap: Snapshot) {
        *self.snapshot.write().unwrap() = Arc::new(snap);
        (self.wake)();
    }

    fn notice(&self, msg: String) {
        self.notices.lock().unwrap().push(msg);
        (self.wake)();
    }
}

/// Handle to the background collectors.
pub struct Monitor {
    shared: Arc<Shared>,
    commands: Option<Sender<Command>>,
}

impl Monitor {
    /// Start sampling. `wake` is called (from a background thread) whenever a
    /// new snapshot or notice is available.
    pub fn start(config: Config, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            snapshot: RwLock::new(Arc::new(Snapshot::default())),
            notices: Mutex::new(Vec::new()),
            wake: Box::new(wake),
        });
        let (tx, rx) = mpsc::channel();
        sampler::spawn(config, shared.clone(), rx);
        Self { shared, commands: Some(tx) }
    }

    /// A monitor that serves a fixed, synthetic snapshot. Useful for
    /// working on the UI on machines without `/proc`.
    pub fn demo() -> Self {
        let shared = Arc::new(Shared {
            snapshot: RwLock::new(Arc::new(demo::snapshot())),
            notices: Mutex::new(Vec::new()),
            wake: Box::new(|| {}),
        });
        Self { shared, commands: None }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.shared.snapshot.read().unwrap().clone()
    }

    /// Messages for the user (e.g. a failed kill), oldest first.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.notices.lock().unwrap())
    }

    pub fn send(&self, cmd: Command) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(cmd);
        }
    }
}

/// Open a directory in the desktop's file manager.
pub fn open_in_file_manager(path: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap in the background so we don't leave a zombie behind.
            std::thread::spawn(move || child.wait());
        });
}
