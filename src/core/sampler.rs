//! The sampling loop: owns all collectors, builds a [`Snapshot`] every
//! interval and executes commands between samples.

use super::cpu::CpuSampler;
use super::disk::{self, DiskState, Filesystem, SharedDisks};
use super::gpu::GpuSampler;
use super::net::{self, Counters, LinkDetails};
use super::ping::{Pinger, Probe};
use super::procs::ProcSampler;
use super::util::{read_string, read_trimmed};
use super::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Instant, SystemTime};

struct PingState {
    host: String,
    history: History,
    method: &'static str,
}

pub fn spawn(config: Config, shared: Arc<Shared>, commands: Receiver<Command>) {
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || run(config, shared, commands))
        .expect("spawn sampler");
}

fn run(config: Config, shared: Arc<Shared>, commands: Receiver<Command>) {
    let n = config.history_len;
    let mut cpu = CpuSampler::new();
    let gpus = GpuSampler::new();
    let mut procs = ProcSampler::new();
    let mut counters = Counters::new();

    let mut snap = Snapshot {
        host: HostInfo {
            hostname: read_trimmed("/proc/sys/kernel/hostname").unwrap_or_default(),
            kernel: read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_default(),
            uptime_secs: 0,
        },
        cpu: CpuStats {
            name: cpu.name.clone(),
            usage: History::new(n),
            temp: cpu.has_temp().then(|| History::new(n)),
            physical_cores: cpu.physical_cores,
            ..Default::default()
        },
        memory: MemStats { usage: History::new(n), ..Default::default() },
        gpus: gpus
            .gpus
            .iter()
            .map(|g| GpuStats {
                name: g.name.clone(),
                vendor: g.vendor,
                usage: History::new(n),
                usage_is_clock: g.usage_is_clock,
                temp: None,
                vram_used: None,
                vram_total: None,
                power_w: None,
                clock_mhz: None,
            })
            .collect(),
        net: NetStats { rx: History::new(n), tx: History::new(n), ..Default::default() },
        interval: config.interval.as_secs_f32(),
        ..Default::default()
    };

    let ping = Arc::new(Mutex::new(PingState {
        host: config.ping_host.clone(),
        history: History::new(n),
        method: "icmp",
    }));
    spawn_pinger(ping.clone(), config.interval);

    let link = Arc::new(Mutex::new((String::new(), LinkDetails::default())));
    let iface_now = Arc::new(Mutex::new(String::new()));
    spawn_link_details(link.clone(), iface_now.clone());

    let disks: SharedDisks = Arc::new(Mutex::new(DiskState::default()));
    let rescan = disk::spawn_scanner(disks.clone());
    disk::spawn_live(disks.clone());
    let mut filesystems: Vec<Filesystem> = Vec::new();

    let mut next = Instant::now();
    let mut tick: u64 = 0;
    loop {
        // ---- sample
        let c = cpu.sample();
        snap.cpu.usage.push(c.usage);
        if let (Some(h), Some(t)) = (snap.cpu.temp.as_mut(), c.temp) {
            h.push(t);
        }
        snap.cpu.per_core = c.per_core;
        snap.cpu.freq_mhz = c.freq_mhz;

        let m = mem::sample();
        snap.memory.usage.push(if m.total > 0 { m.used as f32 / m.total as f32 * 100.0 } else { 0.0 });
        snap.memory.total = m.total;
        snap.memory.used = m.used;
        snap.memory.cached = m.cached;
        snap.memory.swap_total = m.swap_total;
        snap.memory.swap_used = m.swap_used;

        for (g, s) in snap.gpus.iter_mut().zip(gpus.sample()) {
            g.usage.push(s.usage.unwrap_or(0.0));
            if let Some(t) = s.temp {
                g.temp.get_or_insert_with(|| History::new(n)).push(t);
            }
            g.vram_used = s.vram_used;
            g.vram_total = s.vram_total;
            g.power_w = s.power_w;
            g.clock_mhz = s.clock_mhz;
        }

        // Network
        let iface = net::default_iface().unwrap_or_default();
        if iface != snap.net.iface {
            snap.net.kind = net::link_kind(&iface);
            snap.net.iface = iface.clone();
            *iface_now.lock().unwrap() = iface.clone();
        }
        let (rx, tx) = if iface.is_empty() { (0.0, 0.0) } else { counters.rates(&iface) };
        snap.net.rx.push(rx);
        snap.net.tx.push(tx);
        if tick.is_multiple_of(5) {
            snap.net.ipv4 = net::ipv4_of(&iface);
        }
        {
            let l = link.lock().unwrap();
            if l.0 == iface {
                snap.net.name = l.1.name.clone();
                snap.net.link_mbps = l.1.link_mbps;
                snap.net.signal = l.1.signal;
            } else {
                snap.net.name = None;
                snap.net.link_mbps = None;
                snap.net.signal = None;
            }
        }
        {
            let p = ping.lock().unwrap();
            snap.net.ping_host = p.host.clone();
            snap.net.ping = p.history.clone();
            snap.net.ping_method = p.method;
        }

        // Disks
        if tick.is_multiple_of(15) || filesystems.is_empty() {
            filesystems = disk::filesystems().0;
        }
        snap.disks = disk_stats(&filesystems, &disks);

        snap.apps = procs.sample();
        snap.host.uptime_secs = read_string("/proc/uptime")
            .and_then(|s| s.split('.').next()?.parse().ok())
            .unwrap_or(0);

        shared.publish(snap.clone());
        tick += 1;

        // ---- wait, handling commands as they come
        next += config.interval;
        let now = Instant::now();
        if next < now {
            next = now + config.interval; // we fell behind (suspend?), don't burst
        }
        loop {
            let wait = next.saturating_duration_since(Instant::now());
            match commands.recv_timeout(wait) {
                Ok(cmd) => handle(cmd, &shared, &ping, &disks, &rescan),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn handle(
    cmd: Command,
    shared: &Shared,
    ping: &Mutex<PingState>,
    disks: &SharedDisks,
    rescan: &std::sync::mpsc::Sender<()>,
) {
    match cmd {
        Command::Signal { pids, signal } => {
            let mut denied = 0;
            for &pid in &pids {
                if !send_signal(pid, signal) {
                    denied += 1;
                }
            }
            if denied > 0 {
                let what = if pids.len() == 1 { "that process".to_string() } else { format!("{denied} of {} processes", pids.len()) };
                shared.notice(format!("Not permitted to stop {what}"));
            }
        }
        Command::RescanDisks => disk::rescan(disks, rescan),
        Command::SetPingHost(host) => {
            let mut p = ping.lock().unwrap();
            if p.host != host {
                p.host = host;
                let cap = p.history.capacity();
                p.history = History::new(cap);
            }
        }
    }
}

fn send_signal(pid: u32, signal: Signal) -> bool {
    #[cfg(unix)]
    unsafe {
        let sig = match signal {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // Already gone is fine.
        libc::kill(pid as i32, sig) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
        false
    }
}

fn disk_stats(filesystems: &[Filesystem], disks: &SharedDisks) -> Vec<DiskStats> {
    let st = disks.lock().unwrap();
    filesystems
        .iter()
        .filter_map(|fs| {
            let (total, used) = disk::space(&fs.mount_points[0])?;
            let result = st.results.get(&fs.id);
            let mut segments = result.map(|r| r.segments.clone()).unwrap_or_default();
            // What the scan couldn't see.
            let seen: u64 = segments.iter().map(|s| s.size).sum();
            if !segments.is_empty() {
                if seen > used {
                    // Compression/dedup: scale to fit the real usage.
                    let k = used as f64 / seen as f64;
                    for s in &mut segments {
                        s.size = (s.size as f64 * k) as u64;
                    }
                } else if used - seen > used / 100 {
                    segments.push(DiskSegment { path: PathBuf::new(), size: used - seen, kind: SegmentKind::Hidden });
                }
            }
            let (source, live_seen) = st.live.get(&fs.id).copied().unwrap_or_default();
            let scanning = st
                .scanning
                .as_ref()
                .filter(|(id, _)| *id == fs.id)
                .map(|(_, p)| p.load(Ordering::Relaxed))
                .or(live_seen);
            Some(DiskStats {
                id: fs.id.clone(),
                mount_points: fs.mount_points.clone(),
                device: fs.device.clone(),
                fs_type: fs.fs_type.clone(),
                total,
                used,
                segments,
                scanning,
                scanned_ago: result.and_then(|r| SystemTime::now().duration_since(r.at).ok()).map(|d| d.as_secs()),
                source,
            })
        })
        .collect()
}

fn spawn_pinger(state: Arc<Mutex<PingState>>, interval: Duration) {
    std::thread::Builder::new()
        .name("ping".into())
        .spawn(move || {
            let mut pinger = Pinger::new(state.lock().unwrap().host.clone());
            loop {
                let host = state.lock().unwrap().host.clone();
                if host != pinger.host() {
                    pinger = Pinger::new(host.clone());
                }
                let start = Instant::now();
                let timeout = interval.max(Duration::from_millis(900));
                let v = match pinger.probe(timeout) {
                    Probe::Ok(ms) => ms,
                    Probe::Lost => f32::NAN,
                };
                {
                    let mut s = state.lock().unwrap();
                    if s.host == host {
                        s.history.push(v);
                        s.method = pinger.method;
                    }
                }
                std::thread::sleep(interval.saturating_sub(start.elapsed()));
            }
        })
        .expect("spawn pinger");
}

fn spawn_link_details(out: Arc<Mutex<(String, LinkDetails)>>, iface: Arc<Mutex<String>>) {
    std::thread::Builder::new()
        .name("link".into())
        .spawn(move || {
            let mut last = String::new();
            let mut since = Instant::now();
            loop {
                let cur = iface.lock().unwrap().clone();
                if !cur.is_empty() && (cur != last || since.elapsed() >= Duration::from_secs(10)) {
                    let d = net::link_details(&cur, net::link_kind(&cur));
                    *out.lock().unwrap() = (cur.clone(), d);
                    last = cur;
                    since = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .expect("spawn link details");
}
