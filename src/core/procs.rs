//! Processes from `/proc`, grouped into applications.
//!
//! A process starts a new group when it is the top of something the user
//! thinks of as "one app":
//! - its parent lives in a different cgroup (systemd puts each launched app
//!   in its own `app-*.scope`/`.service`),
//! - its parent is part of the desktop/session plumbing (shell, compositor,
//!   systemd), or runs as another user,
//! - it is the top of a Steam game. Steam launches games through its `reaper`
//!   with `SteamLaunch AppId=N`, and everything below carries `SteamAppId`
//!   in its environment. Games inherit Steam's cgroup, which is why monitors
//!   that only group by cgroup bury them inside "Steam".

use super::apps::{self, DesktopNames};
use super::steam::{self, SteamLibrary};
use super::util::{clk_tck, current_uid, page_size, read_into};
use super::{AppGroup, AppKind, ProcInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Parent processes whose children are separate applications.
const SESSION_PARENTS: &[&str] = &[
    "systemd", "init", "kthreadd", "plasmashell", "kwin_wayland", "kwin_x11", "kwin_wayland_wr", "gnome-shell",
    "ksmserver", "startplasma-way", "startplasma-x11", "plasma_session", "gnome-session-b", "gnome-session-c",
    "xdg-desktop-por", "sddm", "sddm-helper", "gdm-session-wor", "gdm-wayland-ses", "gdm-x-session", "lightdm",
    "xfce4-session", "xfce4-panel", "lxsession", "lxqt-session", "sway", "Hyprland", "niri", "river", "i3", "bspwm",
    "openbox", "labwc", "wayfire", "kded5", "kded6", "krunner", "dbus-daemon", "dbus-broker", "dbus-broker-lau",
    "runsv", "s6-supervise", "cosmic-session", "cosmic-launcher", "cosmic-panel", "cosmic-comp", "Xwayland", "Xorg",
    "mate-session", "cinnamon-sessio", "cinnamon", "budgie-panel", "waybar", "rofi", "wofi", "fuzzel", "kickoff",
    "systemd-run", "kstart", "kstart5", "kioclient", "kioclient5", "gio", "xdg-open",
];

/// Processes that are plumbing themselves; their groups count as background.
const PLUMBING: &[&str] = &[
    "plasmashell", "kwin_wayland", "kwin_x11", "gnome-shell", "ksmserver", "kded5", "kded6", "Xwayland", "Xorg",
    "xdg-desktop-por", "pipewire", "wireplumber", "pipewire-pulse", "dbus-daemon", "dbus-broker", "at-spi-bus-laun",
];

struct Static {
    starttime: u64,
    uid: u32,
    comm: String,
    name: String,
    cmdline: Arc<str>,
    cgroup: Arc<str>,
    steam_id: Option<u32>,
    refreshed_tick: u64,
}

struct Raw {
    pid: u32,
    ppid: u32,
    starttime: u64,
    ticks: u64,
    mem: u64,
    io: Option<u64>,
}

pub struct ProcSampler {
    my_uid: u32,
    clk: f64,
    page: u64,
    ncpu: f64,
    tick: u64,
    statics: HashMap<u32, Static>,
    /// pid -> (starttime, cpu ticks, io bytes)
    prev: HashMap<u32, (u64, u64, Option<u64>)>,
    prev_time: Instant,
    steam: SteamLibrary,
    desktop: DesktopNames,
    names: HashMap<String, String>,
    buf: Vec<u8>,
}

impl ProcSampler {
    pub fn new() -> Self {
        Self {
            my_uid: current_uid(),
            clk: clk_tck(),
            page: page_size(),
            ncpu: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64,
            tick: 0,
            statics: HashMap::new(),
            prev: HashMap::new(),
            prev_time: Instant::now(),
            steam: SteamLibrary::default(),
            desktop: DesktopNames::default(),
            names: HashMap::new(),
            buf: Vec::with_capacity(4096),
        }
    }

    pub fn sample(&mut self) -> Vec<AppGroup> {
        self.tick += 1;
        let now = Instant::now();
        let dt = now.duration_since(self.prev_time).as_secs_f64().max(1e-3);
        self.prev_time = now;

        let raws = self.read_all();

        // Per-process rates.
        let mut cpu = Vec::with_capacity(raws.len());
        let mut io = Vec::with_capacity(raws.len());
        let mut next_prev = HashMap::with_capacity(raws.len());
        for r in &raws {
            let (c, i) = match self.prev.get(&r.pid) {
                Some(&(st, ticks, pio)) if st == r.starttime => {
                    let c = r.ticks.saturating_sub(ticks) as f64 / self.clk / dt / self.ncpu * 100.0;
                    let i = match (r.io, pio) {
                        (Some(a), Some(b)) => a.saturating_sub(b) as f64 / dt,
                        _ => 0.0,
                    };
                    (c.min(100.0) as f32, i)
                }
                _ => (0.0, 0.0),
            };
            cpu.push(c);
            io.push(i);
            next_prev.insert(r.pid, (r.starttime, r.ticks, r.io));
        }
        self.prev = next_prev;
        self.statics.retain(|pid, _| self.prev.contains_key(pid));

        self.group(&raws, &cpu, &io)
    }

    fn read_all(&mut self) -> Vec<Raw> {
        let mut out = Vec::with_capacity(512);
        let Ok(rd) = std::fs::read_dir("/proc") else { return out };
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            if let Some(r) = self.read_one(pid) {
                out.push(r);
            }
        }
        out
    }

    fn read_one(&mut self, pid: u32) -> Option<Raw> {
        let dir = PathBuf::from(format!("/proc/{pid}"));
        if !read_into(&dir.join("stat"), &mut self.buf) {
            return None;
        }
        let stat = std::str::from_utf8(&self.buf).ok()?;
        let open = stat.find('(')?;
        let close = stat.rfind(')')?;
        let comm = stat[open + 1..close].to_string();
        let f: Vec<&str> = stat[close + 2..].split_ascii_whitespace().collect();
        if f.len() < 20 {
            return None;
        }
        let state = f[0];
        let ppid: u32 = f[1].parse().ok()?;
        let flags: u64 = f[6].parse().unwrap_or(0);
        const PF_KTHREAD: u64 = 0x0020_0000;
        if flags & PF_KTHREAD != 0 || pid == 2 || ppid == 2 || state == "Z" {
            return None;
        }
        let ticks = f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0);
        let starttime: u64 = f[19].parse().unwrap_or(0);

        let mem = if read_into(&dir.join("statm"), &mut self.buf) {
            let s = std::str::from_utf8(&self.buf).unwrap_or("");
            let mut it = s.split_ascii_whitespace().skip(1).map(|x| x.parse::<u64>().unwrap_or(0));
            let resident = it.next().unwrap_or(0);
            let shared = it.next().unwrap_or(0);
            // Anonymous resident memory: what the process itself allocated.
            resident.saturating_sub(shared) * self.page
        } else {
            0
        };

        let stale = match self.statics.get(&pid) {
            Some(s) if s.starttime == starttime => self.tick - s.refreshed_tick >= 5,
            _ => true,
        };
        if stale {
            self.refresh_static(pid, &dir, starttime, comm);
        }
        let st = self.statics.get(&pid)?;

        let io = if st.uid == self.my_uid || self.my_uid == 0 {
            if read_into(&dir.join("io"), &mut self.buf) {
                let s = std::str::from_utf8(&self.buf).unwrap_or("");
                let get = |k: &str| {
                    s.lines()
                        .find_map(|l| l.strip_prefix(k))
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .unwrap_or(0)
                };
                Some(get("read_bytes:") + get("write_bytes:"))
            } else {
                None
            }
        } else {
            None
        };

        Some(Raw { pid, ppid, starttime, ticks, mem, io })
    }

    fn refresh_static(&mut self, pid: u32, dir: &std::path::Path, starttime: u64, comm: String) {
        let tick = self.tick;
        if let Some(s) = self.statics.get_mut(&pid) {
            if s.starttime == starttime {
                // Only the cgroup moves after launch (apps get placed in
                // their scope shortly after they start).
                if read_into(&dir.join("cgroup"), &mut self.buf) {
                    let cg = parse_cgroup(&self.buf);
                    if *s.cgroup != *cg {
                        s.cgroup = cg.into();
                    }
                }
                s.refreshed_tick = tick;
                return;
            }
        }

        #[cfg(unix)]
        let uid = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(dir).map(|m| m.uid()).unwrap_or(u32::MAX)
        };
        #[cfg(not(unix))]
        let uid = 0;

        let cmdline = if read_into(&dir.join("cmdline"), &mut self.buf) {
            let raw = String::from_utf8_lossy(&self.buf);
            raw.trim_end_matches('\0').to_string()
        } else {
            String::new()
        };
        let args: Vec<&str> = cmdline.split('\0').collect();
        let cgroup = if read_into(&dir.join("cgroup"), &mut self.buf) { parse_cgroup(&self.buf) } else { String::new() };

        let mut steam_id = steam::app_id_from_cmdline(&args);
        if steam_id.is_none() && uid == self.my_uid && read_into(&dir.join("environ"), &mut self.buf) {
            steam_id = steam::app_id_from_environ(&self.buf);
        }

        let name = display_name(&comm, args.first().copied().unwrap_or(""));
        let cmdline: Arc<str> = if cmdline.is_empty() { comm.as_str().into() } else { cmdline.replace('\0', " ").into() };
        self.statics.insert(
            pid,
            Static { starttime, uid, comm, name, cmdline, cgroup: cgroup.into(), steam_id, refreshed_tick: tick },
        );
    }

    fn group(&mut self, raws: &[Raw], cpu: &[f32], io: &[f64]) -> Vec<AppGroup> {
        let index: HashMap<u32, usize> = raws.iter().enumerate().map(|(i, r)| (r.pid, i)).collect();
        let st = |i: usize| &self.statics[&raws[i].pid];
        let parent = |i: usize| index.get(&raws[i].ppid).copied();

        let is_root = |i: usize| -> bool {
            let Some(p) = parent(i) else { return true };
            if raws[p].pid <= 2 {
                return true;
            }
            let (me, pa) = (st(i), st(p));
            (me.steam_id.is_some() && me.steam_id != pa.steam_id)
                || me.uid != pa.uid
                || me.cgroup != pa.cgroup
                || SESSION_PARENTS.contains(&pa.comm.as_str())
        };

        // Resolve each process's root, memoized along the walked path.
        let mut root: Vec<Option<usize>> = vec![None; raws.len()];
        let mut path = Vec::new();
        for i in 0..raws.len() {
            let mut cur = i;
            path.clear();
            let r = loop {
                if let Some(r) = root[cur] {
                    break r;
                }
                path.push(cur);
                if is_root(cur) || path.len() > 256 {
                    break cur;
                }
                cur = parent(cur).unwrap();
            };
            for &p in &path {
                root[p] = Some(r);
            }
        }

        // Children lists, for tree ordering inside a group.
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, r) in root.iter().enumerate() {
            if *r != Some(i) {
                if let Some(p) = parent(i) {
                    children.entry(p).or_default().push(i);
                }
            }
        }

        // Group roots by key.
        let mut groups: HashMap<String, (Vec<usize>, AppKind, Option<String>)> = HashMap::new();
        for (i, r) in root.iter().enumerate() {
            if *r != Some(i) {
                continue;
            }
            let s = st(i);
            let leaf = s.cgroup.rsplit('/').next().unwrap_or("");
            let unit_app = apps::app_id_from_unit(leaf);
            let (key, kind) = if let Some(id) = s.steam_id {
                (format!("game:{id}"), AppKind::Game)
            } else if s.uid != self.my_uid && self.my_uid != 0 {
                if leaf.ends_with(".service") {
                    (format!("cg:{}", s.cgroup), AppKind::System)
                } else {
                    (format!("p:{}:{}", raws[i].pid, raws[i].starttime), AppKind::System)
                }
            } else if unit_app.is_some() {
                (format!("cg:{}", s.cgroup), AppKind::App)
            } else if leaf.ends_with(".service") {
                (format!("cg:{}", s.cgroup), AppKind::Background)
            } else {
                // Started by the service manager and not a known desktop app:
                // a daemon. Started by a launcher or shell: an app.
                let by_manager = parent(i).is_none_or(|p| raws[p].pid == 1 || st(p).comm == "systemd");
                let daemon = PLUMBING.contains(&s.comm.as_str())
                    || (by_manager && self.desktop.by_exec(&s.name).is_none());
                let kind = if daemon { AppKind::Background } else { AppKind::App };
                (format!("p:{}:{}", raws[i].pid, raws[i].starttime), kind)
            };
            let entry = groups.entry(key).or_insert_with(|| (Vec::new(), kind, unit_app));
            entry.0.push(i);
        }

        let mut out = Vec::with_capacity(groups.len());
        for (key, (mut roots, kind, unit_app)) in groups {
            roots.sort_by_key(|&i| raws[i].starttime);
            let mut procs = Vec::new();
            let mut stack: Vec<(usize, u8)> = roots.iter().rev().map(|&i| (i, 0)).collect();
            while let Some((i, depth)) = stack.pop() {
                let s = st(i);
                procs.push(ProcInfo {
                    pid: raws[i].pid,
                    name: s.name.clone(),
                    cmdline: s.cmdline.clone(),
                    depth,
                    cpu: cpu[i],
                    mem: raws[i].mem,
                    io: io[i],
                });
                if let Some(ch) = children.get(&i) {
                    let mut ch = ch.clone();
                    ch.sort_by_key(|&c| std::cmp::Reverse(raws[c].pid));
                    stack.extend(ch.into_iter().map(|c| (c, depth.saturating_add(1))));
                }
            }

            let first = roots[0];
            let name = match self.names.get(&key) {
                Some(n) => n.clone(),
                None => {
                    let root_name = st(first).name.clone();
                    let steam_id = st(first).steam_id;
                    let n = if let Some(id) = steam_id.filter(|_| kind == AppKind::Game) {
                        self.steam.name(id)
                    } else if let Some(id) = &unit_app {
                        self.desktop
                            .by_app_id(id)
                            .or_else(|| self.desktop.by_exec(&root_name))
                            .unwrap_or_else(|| apps::prettify_id(id))
                    } else if kind == AppKind::System && key.starts_with("cg:") {
                        let leaf = key.rsplit('/').next().unwrap_or("");
                        leaf.trim_end_matches(".service").to_string()
                    } else {
                        self.desktop.by_exec(&root_name).unwrap_or(root_name)
                    };
                    self.names.insert(key.clone(), n.clone());
                    n
                }
            };

            out.push(AppGroup {
                cpu: procs.iter().map(|p| p.cpu).sum::<f32>().min(100.0),
                mem: procs.iter().map(|p| p.mem).sum(),
                io: procs.iter().map(|p| p.io).sum(),
                key,
                name,
                kind,
                procs,
            });
        }
        if self.names.len() > 4096 {
            self.names.clear();
        }
        out
    }
}

fn parse_cgroup(buf: &[u8]) -> String {
    let s = String::from_utf8_lossy(buf);
    // cgroup v2 unified line is "0::/path"; fall back to the last line.
    s.lines()
        .find_map(|l| l.strip_prefix("0::"))
        .or_else(|| s.lines().last().and_then(|l| l.splitn(3, ':').nth(2)))
        .unwrap_or("")
        .to_string()
}

/// `comm` is truncated to 15 bytes; prefer the full argv[0] basename when it
/// is clearly the same program.
fn display_name(comm: &str, argv0: &str) -> String {
    let base = argv0.rsplit(['/', '\\']).next().unwrap_or("");
    let base = base.split(' ').next().unwrap_or(base);
    if !base.is_empty() && base.len() > comm.len() && base.starts_with(comm) {
        base.to_string()
    } else {
        comm.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(display_name("steamwebhelper", "/home/u/.steam/ubuntu12_64/steamwebhelper"), "steamwebhelper");
        assert_eq!(display_name("EldenRing.exe", "Z:\\games\\ELDEN RING\\Game\\EldenRing.exe"), "EldenRing.exe");
        assert_eq!(display_name("xdg-desktop-por", "/usr/libexec/xdg-desktop-portal-kde"), "xdg-desktop-portal-kde");
    }

    #[test]
    fn cgroups() {
        assert_eq!(parse_cgroup(b"0::/user.slice/app.slice/app-steam-1.scope\n"), "/user.slice/app.slice/app-steam-1.scope");
    }
}
