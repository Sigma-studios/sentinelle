//! A plausible, fixed snapshot for UI work on machines without `/proc`.
//!
//! Disks look as répertoire serves them (one live, one reconciling after a
//! restart, one stale after missed events). `SENTINELLE_DEMO=scan` shows
//! them as sentinelle's own scanner does instead.

use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32) / (1u64 << 31) as f32
    }
}

fn series(rng: &mut Rng, n: usize, base: f32, swing: f32, noise: f32, lo: f32, hi: f32) -> History {
    let mut h = History::new(n);
    let mut v = base;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let target = base + swing * (t * 9.0).sin() * (t * 2.3 + 0.4).cos();
        v += (target - v) * 0.35 + (rng.next() - 0.5) * noise;
        h.push(v.clamp(lo, hi));
    }
    h
}

fn proc(pid: u32, name: &str, depth: u8, cpu: f32, mem_mb: u64) -> ProcInfo {
    ProcInfo { pid, name: name.into(), cmdline: format!("/usr/bin/{name}").into(), depth, cpu, mem: mem_mb << 20, io: 0.0 }
}

fn group(key: &str, name: &str, kind: AppKind, procs: Vec<ProcInfo>, io: f64) -> AppGroup {
    AppGroup {
        key: key.into(),
        name: name.into(),
        kind,
        cpu: procs.iter().map(|p| p.cpu).sum(),
        mem: procs.iter().map(|p| p.mem).sum(),
        io,
        procs,
    }
}

pub fn snapshot() -> Snapshot {
    let n = 90;
    let mut r = Rng(7);
    const GB: u64 = 1 << 30;
    let per_core = (0..16).map(|i| ((i as f32 * 1.7).sin().abs() * 70.0 + r.next() * 25.0).min(100.0)).collect();
    let mut ping = series(&mut r, n, 14.0, 4.0, 6.0, 8.0, 80.0);
    ping.push(f32::NAN);
    ping.push(16.0);
    ping.push(13.2);

    Snapshot {
        interval: 1.0,
        host: HostInfo { hostname: "atelier".into(), kernel: "6.17.4-arch1-1".into(), uptime_secs: 3 * 86400 + 4 * 3600 + 720 },
        cpu: CpuStats {
            name: "AMD Ryzen 7 7800X3D".into(),
            usage: series(&mut r, n, 34.0, 22.0, 18.0, 2.0, 100.0),
            temp: Some(series(&mut r, n, 62.0, 9.0, 3.0, 30.0, 95.0)),
            freq_mhz: Some(4712.0),
            per_core,
            physical_cores: 8,
        },
        memory: MemStats {
            total: 32 * GB,
            used: 19 * GB + GB / 3,
            cached: 8 * GB,
            swap_total: 8 * GB,
            swap_used: GB / 5,
            usage: series(&mut r, n, 58.0, 4.0, 1.5, 0.0, 100.0),
        },
        gpus: vec![
            GpuStats {
                name: "Radeon RX 7900 XTX".into(),
                vendor: GpuVendor::Amd,
                usage: series(&mut r, n, 78.0, 18.0, 12.0, 0.0, 100.0),
                usage_is_clock: false,
                temp: Some(series(&mut r, n, 71.0, 6.0, 2.0, 30.0, 100.0)),
                vram_used: Some(14 * GB + GB / 2),
                vram_total: Some(24 * GB),
                power_w: Some(287.0),
                clock_mhz: Some(2614),
            },
            GpuStats {
                name: "Radeon 780M".into(),
                vendor: GpuVendor::Amd,
                usage: series(&mut r, n, 4.0, 3.0, 4.0, 0.0, 100.0),
                usage_is_clock: false,
                temp: Some(series(&mut r, n, 48.0, 2.0, 1.0, 30.0, 100.0)),
                vram_used: Some(GB / 2),
                vram_total: Some(2 * GB),
                power_w: Some(6.4),
                clock_mhz: Some(800),
            },
        ],
        disks: disks(),
        net: NetStats {
            iface: "wlan0".into(),
            kind: LinkKind::Wireless,
            name: Some("Maison 5G".into()),
            ipv4: Some("192.168.1.42".into()),
            link_mbps: Some(1201.0),
            signal: Some(78),
            rx: series(&mut r, n, 4.0e6, 3.5e6, 3.0e6, 0.0, 1e9),
            tx: series(&mut r, n, 0.6e6, 0.5e6, 0.5e6, 0.0, 1e9),
            ping_host: "1.1.1.1".into(),
            ping,
            ping_method: "icmp",
        },
        apps: vec![
            group(
                "game:1245620",
                "ELDEN RING",
                AppKind::Game,
                vec![
                    proc(48211, "reaper", 0, 0.0, 2),
                    proc(48215, "pressure-vessel", 1, 0.1, 12),
                    proc(48290, "wineserver", 2, 0.8, 38),
                    proc(48302, "eldenring.exe", 2, 21.4, 7420),
                ],
                12.0e6,
            ),
            group(
                "cg:/app-steam",
                "Steam",
                AppKind::App,
                vec![
                    proc(3021, "steam", 0, 0.6, 410),
                    proc(3188, "steamwebhelper", 1, 0.4, 380),
                    proc(3201, "steamwebhelper", 2, 0.2, 290),
                ],
                0.4e6,
            ),
            group(
                "cg:/app-firefox",
                "Firefox",
                AppKind::App,
                vec![
                    proc(2210, "firefox", 0, 2.1, 980),
                    proc(2290, "Isolated Web Co", 1, 1.4, 420),
                    proc(2291, "Isolated Web Co", 1, 0.3, 230),
                    proc(2302, "RDD Process", 1, 0.1, 60),
                ],
                0.1e6,
            ),
            group("cg:/app-konsole", "Konsole", AppKind::App, vec![proc(1880, "konsole", 0, 0.2, 90), proc(1893, "zsh", 1, 0.0, 6)], 0.0),
            group("cg:/app-discord", "Discord", AppKind::App, vec![proc(4410, "Discord", 0, 0.9, 610)], 0.05e6),
            group("cg:/pipewire", "PipeWire", AppKind::Background, vec![proc(1402, "pipewire", 0, 0.4, 24)], 0.0),
            group("p:plasmashell", "Plasma Workspace", AppKind::Background, vec![proc(1350, "plasmashell", 0, 1.1, 420)], 0.0),
            group("cg:/system/NetworkManager", "NetworkManager", AppKind::System, vec![proc(812, "NetworkManager", 0, 0.0, 18)], 0.0),
            group("cg:/system/sddm", "sddm", AppKind::System, vec![proc(901, "sddm", 0, 0.0, 9)], 0.0),
        ],
    }
}

fn disks() -> Vec<DiskStats> {
    const GB: u64 = 1 << 30;
    let scan = std::env::var("SENTINELLE_DEMO").is_ok_and(|v| v == "scan");
    let seg = |p: &str, gb: u64, kind| DiskSegment { path: p.into(), size: gb * GB, kind };
    let root = DiskStats {
        id: "/dev/nvme0n1p2".into(),
        mount_points: vec!["/".into(), "/home".into()],
        device: "nvme0n1p2".into(),
        fs_type: "btrfs".into(),
        total: 1863 * GB,
        used: 1214 * GB,
        segments: vec![
            seg("/home/user/.local/share/Steam", 486, SegmentKind::Dir),
            seg("/home/user/Videos", 241, SegmentKind::Dir),
            seg("/home/user", 168, SegmentKind::Rest),
            seg("/home/user/.cache", 129, SegmentKind::Dir),
            seg("/usr", 122, SegmentKind::Dir),
            seg("", 46, SegmentKind::Other),
            seg("", 22, SegmentKind::Hidden),
        ],
        scanning: None,
        scanned_ago: Some(if scan { 240 } else { 0 }),
        source: if scan { DiskSource::Scan } else { DiskSource::Live },
    };
    let archive = DiskStats {
        id: "/dev/sda1".into(),
        mount_points: vec!["/mnt/archive".into()],
        device: "sda1".into(),
        fs_type: "ext4".into(),
        total: 3726 * GB,
        used: 1102 * GB,
        segments: if scan {
            vec![]
        } else {
            vec![
                seg("/mnt/archive/photos", 512, SegmentKind::Dir),
                seg("/mnt/archive/backups", 380, SegmentKind::Dir),
                seg("/mnt/archive", 170, SegmentKind::Rest),
                seg("", 40, SegmentKind::Other),
            ]
        },
        scanning: scan.then_some(388 * GB),
        scanned_ago: (!scan).then_some(0),
        source: if scan { DiskSource::Scan } else { DiskSource::Reconciling { progress: 0.63 } },
    };
    let mut disks = vec![root, archive];
    if !scan {
        disks.push(DiskStats {
            id: "/dev/sdb1".into(),
            mount_points: vec!["/mnt/games".into()],
            device: "sdb1".into(),
            fs_type: "ext4".into(),
            total: 931 * GB,
            used: 702 * GB,
            segments: vec![
                seg("/mnt/games/SteamLibrary/steamapps/common", 611, SegmentKind::Dir),
                seg("/mnt/games/SteamLibrary/steamapps/shadercache", 71, SegmentKind::Dir),
                seg("", 20, SegmentKind::Other),
            ],
            scanning: None,
            scanned_ago: Some(0),
            source: DiskSource::Stale { progress: 0.18 },
        });
        disks.push(DiskStats {
            id: "/dev/sdc1".into(),
            mount_points: vec!["/mnt/new".into()],
            device: "sdc1".into(),
            fs_type: "xfs".into(),
            total: 1863 * GB,
            used: 240 * GB,
            segments: vec![],
            scanning: Some(91 * GB),
            scanned_ago: None,
            source: DiskSource::Indexing { progress: 0.38 },
        });
    }
    disks
}
