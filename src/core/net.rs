//! Default-route interface, its traffic counters, and slowly changing
//! details (network name, link speed) that need external tools.

use super::util::{read_string, read_trimmed, read_u64};
use super::LinkKind;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Interface carrying the default IPv4 route (lowest metric), else the
/// first interface that is up and not loopback.
pub fn default_iface() -> Option<String> {
    let routes = read_string("/proc/net/route").unwrap_or_default();
    let best = routes
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() > 6 && f[1] == "00000000").then(|| (f[6].parse::<u32>().unwrap_or(u32::MAX), f[0].to_string()))
        })
        .min();
    if let Some((_, iface)) = best {
        return Some(iface);
    }
    let rd = std::fs::read_dir("/sys/class/net").ok()?;
    rd.flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n != "lo" && read_trimmed(format!("/sys/class/net/{n}/operstate")).as_deref() == Some("up"))
}

pub fn link_kind(iface: &str) -> LinkKind {
    let base = Path::new("/sys/class/net").join(iface);
    if base.join("wireless").exists() || base.join("phy80211").exists() {
        LinkKind::Wireless
    } else if base.join("device").exists() {
        LinkKind::Wired
    } else {
        LinkKind::Other
    }
}

pub struct Counters {
    prev: Option<(String, u64, u64, Instant)>,
}

impl Counters {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// Returns (rx, tx) in bytes per second since the previous call.
    pub fn rates(&mut self, iface: &str) -> (f32, f32) {
        let base = Path::new("/sys/class/net").join(iface).join("statistics");
        let rx = read_u64(base.join("rx_bytes")).unwrap_or(0);
        let tx = read_u64(base.join("tx_bytes")).unwrap_or(0);
        let now = Instant::now();
        let out = match &self.prev {
            Some((name, prx, ptx, t)) if name == iface => {
                let dt = now.duration_since(*t).as_secs_f32().max(1e-3);
                (rx.saturating_sub(*prx) as f32 / dt, tx.saturating_sub(*ptx) as f32 / dt)
            }
            _ => (0.0, 0.0),
        };
        self.prev = Some((iface.to_string(), rx, tx, now));
        out
    }
}

pub fn ipv4_of(iface: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut cur = ifap;
        let mut out = None;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET
                && std::ffi::CStr::from_ptr(ifa.ifa_name).to_bytes() == iface.as_bytes()
            {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                out = Some(std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)).to_string());
                break;
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        out
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
        None
    }
}

#[derive(Default, Clone)]
pub struct LinkDetails {
    pub name: Option<String>,
    pub link_mbps: Option<f32>,
    pub signal: Option<u8>,
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Network name, link rate and signal. Spawns `iw`/`nmcli`, so call rarely.
pub fn link_details(iface: &str, kind: LinkKind) -> LinkDetails {
    let mut d = LinkDetails::default();
    if kind == LinkKind::Wireless {
        if let Some(out) = run("iw", &["dev", iface, "link"]) {
            for line in out.lines().map(str::trim) {
                if let Some(v) = line.strip_prefix("SSID:") {
                    d.name = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("tx bitrate:") {
                    d.link_mbps = v.split_whitespace().next().and_then(|x| x.parse().ok());
                } else if let Some(v) = line.strip_prefix("signal:") {
                    let dbm: f32 = v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(-100.0);
                    // -90 dBm (unusable) .. -30 dBm (excellent)
                    d.signal = Some(((dbm + 90.0) / 60.0 * 100.0).clamp(0.0, 100.0) as u8);
                }
            }
        }
        if d.signal.is_none() {
            d.signal = proc_wireless_quality(iface);
        }
    } else {
        d.link_mbps = read_u64(format!("/sys/class/net/{iface}/speed")).filter(|&s| s > 0 && s < 1_000_000).map(|s| s as f32);
    }
    if d.name.is_none() {
        d.name = nmcli_connection(iface);
    }
    if d.name.is_none() && kind == LinkKind::Wireless {
        d.name = run("iwgetid", &["-r"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    d
}

fn nmcli_connection(iface: &str) -> Option<String> {
    let out = run("nmcli", &["-t", "-f", "NAME,DEVICE", "connection", "show", "--active"])?;
    for line in out.lines() {
        // Fields are ':'-separated with '\:' escapes.
        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut esc = false;
        for c in line.chars() {
            match (esc, c) {
                (true, c) => {
                    cur.push(c);
                    esc = false;
                }
                (false, '\\') => esc = true,
                (false, ':') => fields.push(std::mem::take(&mut cur)),
                (false, c) => cur.push(c),
            }
        }
        fields.push(cur);
        if fields.len() >= 2 && fields[1] == iface {
            return Some(fields[0].clone());
        }
    }
    None
}

fn proc_wireless_quality(iface: &str) -> Option<u8> {
    let text = read_string("/proc/net/wireless")?;
    let line = text.lines().find(|l| l.trim_start().starts_with(&format!("{iface}:")))?;
    let q: f32 = line.split_whitespace().nth(2)?.trim_end_matches('.').parse().ok()?;
    Some((q / 70.0 * 100.0).clamp(0.0, 100.0) as u8)
}
