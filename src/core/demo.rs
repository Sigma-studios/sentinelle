//! A plausible, fixed snapshot for UI work on machines without `/proc`.

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
        disks: vec![
            DiskStats {
                id: "/dev/nvme0n1p2".into(),
                mount_points: vec!["/".into(), "/home".into()],
                device: "nvme0n1p2".into(),
                fs_type: "btrfs".into(),
                total: 1863 * GB,
                used: 1214 * GB,
                segments: vec![
                    DiskSegment { path: "/home/user/.local/share/Steam".into(), size: 486 * GB, kind: SegmentKind::Dir },
                    DiskSegment { path: "/home/user/Videos".into(), size: 241 * GB, kind: SegmentKind::Dir },
                    DiskSegment { path: "/home/user".into(), size: 168 * GB, kind: SegmentKind::Rest },
                    DiskSegment { path: "/home/user/.cache".into(), size: 129 * GB, kind: SegmentKind::Dir },
                    DiskSegment { path: "/usr".into(), size: 122 * GB, kind: SegmentKind::Dir },
                    DiskSegment { path: "".into(), size: 46 * GB, kind: SegmentKind::Other },
                    DiskSegment { path: "".into(), size: 22 * GB, kind: SegmentKind::Hidden },
                ],
                scanning: None,
                scanned_ago: Some(240),
            },
            DiskStats {
                id: "/dev/sda1".into(),
                mount_points: vec!["/mnt/archive".into()],
                device: "sda1".into(),
                fs_type: "ext4".into(),
                total: 3726 * GB,
                used: 1102 * GB,
                segments: vec![],
                scanning: Some(388 * GB),
                scanned_ago: None,
            },
        ],
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
