mod core;
mod ui;

use std::time::Duration;

const HELP: &str = "sentinelle — a lightweight system monitor

USAGE: sentinelle [OPTIONS]

OPTIONS:
    --ping <HOST>        Host to measure latency against (default 1.1.1.1)
    --interval <MS>      Sampling interval in milliseconds (default 1000)
    --dump               Print a few samples to stdout instead of opening a window
    --demo               Show synthetic data (for UI work)
    -h, --help           Show this help";

fn main() {
    let mut config = core::Config::default();
    let mut demo = false;
    let mut dump = false;
    if let Ok(host) = std::env::var("SENTINELLE_PING") {
        config.ping_host = host;
    }
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ping" => config.ping_host = args.next().unwrap_or_else(|| usage_error("--ping needs a host")),
            "--interval" => {
                let ms: u64 = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage_error("--interval needs milliseconds"));
                config.interval = Duration::from_millis(ms.clamp(100, 10_000));
            }
            "--demo" => demo = true,
            "--dump" => dump = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            other => usage_error(&format!("unknown option {other}")),
        }
    }

    if dump {
        dump_samples(config);
        return;
    }
    if let Err(e) = ui::run(config, demo) {
        eprintln!("sentinelle: {e}");
        std::process::exit(1);
    }
}

fn usage_error(msg: &str) -> ! {
    eprintln!("sentinelle: {msg}\n\n{HELP}");
    std::process::exit(2);
}

/// Headless mode: handy to check what the collectors see on a machine.
fn dump_samples(config: core::Config) {
    let interval = config.interval;
    let monitor = core::Monitor::start(config, || {});
    std::thread::sleep(interval * 3 + Duration::from_millis(200));
    let s = monitor.snapshot();
    println!("host     {} / {} / up {}s", s.host.hostname, s.host.kernel, s.host.uptime_secs);
    println!(
        "cpu      {} ({} cores) usage {:.1}% temp {:?} freq {:?}",
        s.cpu.name,
        s.cpu.physical_cores,
        s.cpu.usage.last().unwrap_or(0.0),
        s.cpu.temp.as_ref().and_then(|t| t.last()),
        s.cpu.freq_mhz
    );
    println!("memory   {} / {} MiB", s.memory.used >> 20, s.memory.total >> 20);
    for g in &s.gpus {
        println!(
            "gpu      {} {:?} usage {:?} temp {:?} vram {:?}/{:?}",
            g.name,
            g.vendor,
            g.usage.last(),
            g.temp.as_ref().and_then(|t| t.last()),
            g.vram_used,
            g.vram_total
        );
    }
    for d in &s.disks {
        println!("disk     {} {:?} {} {}/{} GiB", d.device, d.mount_points, d.fs_type, d.used >> 30, d.total >> 30);
        print_segments(d);
    }
    println!(
        "net      {} {:?} {:?} {:?} ping {:?} via {}",
        s.net.iface,
        s.net.kind,
        s.net.name,
        s.net.ipv4,
        s.net.ping.last(),
        s.net.ping_method
    );
    // SENTINELLE_DUMP_FOR=<secs>: keep watching storage, printing it again
    // whenever it changes (handy to see répertoire's live updates).
    let watch: u64 = std::env::var("SENTINELLE_DUMP_FOR").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let until = std::time::Instant::now() + Duration::from_secs(watch);
    let mut last = disks_line(&s.disks);
    while std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(500));
        let now = monitor.snapshot();
        if disks_line(&now.disks) != last {
            last = disks_line(&now.disks);
            for d in &now.disks {
                println!("disk     {:?}", d.mount_points);
                print_segments(d);
            }
        }
    }
    for a in &s.apps {
        println!("app      {:<10} {:<32} {:>5.1}% {:>6} MiB  {} procs", format!("{:?}", a.kind), a.name, a.cpu, a.mem >> 20, a.procs.len());
    }
}

fn print_segments(d: &core::DiskStats) {
    println!("           source {:?}, scanning {:?}, scanned {:?} s ago", d.source, d.scanning, d.scanned_ago);
    for seg in &d.segments {
        println!("           {:>8} MiB  {:?} {}", seg.size >> 20, seg.kind, seg.path.display());
    }
}

fn disks_line(disks: &[core::DiskStats]) -> String {
    disks.iter().map(|d| format!("{:?}{:?}", d.source, d.segments.iter().map(|s| (s.size, &s.path)).collect::<Vec<_>>())).collect()
}
