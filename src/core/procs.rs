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
use std::collections::{HashMap, HashSet};
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
    /// utime + stime.
    ticks: u64,
    /// cutime + cstime: lifetime CPU of reaped children, grandchildren included.
    cticks: u64,
    mem: u64,
    /// read_bytes + write_bytes. Like `cticks`, this grows by a child's
    /// lifetime I/O when the child is reaped.
    io: Option<u64>,
}

/// What `read_one` found for a pid. Zombies aren't displayed, but they keep
/// their last live totals until their parent reaps them (see `rates`).
enum Read {
    Live(Raw),
    Zombie { pid: u32, starttime: u64 },
    Skip,
}

/// What the previous sample knew of a process.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Prev {
    starttime: u64,
    ppid: u32,
    ticks: u64,
    cticks: u64,
    io: Option<u64>,
}

impl Prev {
    fn of(r: &Raw) -> Self {
        Self { starttime: r.starttime, ppid: r.ppid, ticks: r.ticks, cticks: r.cticks, io: r.io }
    }
}

/// Timing of the current sample, for `rates`.
struct Clock {
    /// Seconds since the previous sample.
    dt: f64,
    /// /proc/uptime now and at the previous sample, in seconds.
    uptime: f64,
    prev_uptime: Option<f64>,
    clk: f64,
    ncpu: f64,
}

pub struct ProcSampler {
    my_uid: u32,
    clk: f64,
    page: u64,
    ncpu: f64,
    tick: u64,
    statics: HashMap<u32, Static>,
    prev: HashMap<u32, Prev>,
    prev_time: Instant,
    prev_uptime: Option<f64>,
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
            prev_uptime: None,
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

        let uptime = read_uptime(&mut self.buf);
        let (raws, zombies) = self.read_all();

        let clock = Clock { dt, uptime: uptime.unwrap_or(0.0), prev_uptime: self.prev_uptime, clk: self.clk, ncpu: self.ncpu };
        let (cpu, io, next_prev) = rates(&self.prev, &raws, &zombies, &clock);
        self.prev = next_prev;
        // Without uptime, new processes can't be told from missed old ones.
        self.prev_uptime = uptime;
        self.statics.retain(|pid, _| self.prev.contains_key(pid));

        self.group(&raws, &cpu, &io)
    }

    /// Live processes, and zombies as (pid, starttime).
    fn read_all(&mut self) -> (Vec<Raw>, Vec<(u32, u64)>) {
        let mut out = Vec::with_capacity(512);
        let mut zombies = Vec::new();
        let Ok(rd) = std::fs::read_dir("/proc") else { return (out, zombies) };
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            match self.read_one(pid) {
                Read::Live(r) => out.push(r),
                Read::Zombie { pid, starttime } => zombies.push((pid, starttime)),
                Read::Skip => {}
            }
        }
        (out, zombies)
    }

    fn read_one(&mut self, pid: u32) -> Read {
        self.read_one_inner(pid).unwrap_or(Read::Skip)
    }

    fn read_one_inner(&mut self, pid: u32) -> Option<Read> {
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
        if flags & PF_KTHREAD != 0 || pid == 2 || ppid == 2 {
            return None;
        }
        // `f` starts at stat field 3 (state), so field n is f[n - 3].
        let num = |i: usize| f[i].parse::<u64>().unwrap_or(0);
        let starttime = num(19);
        if state == "Z" {
            return Some(Read::Zombie { pid, starttime });
        }
        let ticks = num(11) + num(12);
        let cticks = num(13) + num(14);

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

        Some(Read::Live(Raw { pid, ppid, starttime, ticks, cticks, mem, io }))
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

/// Per-process CPU (% of the whole machine) and I/O (bytes/s) since the
/// previous sample, aligned with `raws`, and the totals to keep for the next one.
///
/// A process seen in both samples is charged its own CPU delta plus the part
/// of its reaped children's CPU (the `cticks` delta) not already charged to
/// them: when a child is reaped the kernel adds its whole lifetime to the
/// parent's `cticks`, and the child's totals as of its last sample were
/// already counted under the child. The same goes for I/O, which the kernel
/// also folds into the parent at reap time.
///
/// A process that started since the previous sample is charged everything it
/// has used, over its age (or `dt`, if shorter).
fn rates(
    prev: &HashMap<u32, Prev>,
    raws: &[Raw],
    zombies: &[(u32, u64)],
    t: &Clock,
) -> (Vec<f32>, Vec<f64>, HashMap<u32, Prev>) {
    let index: HashMap<u32, usize> = raws.iter().enumerate().map(|(i, r)| (r.pid, i)).collect();
    // Same process as in the previous sample (pids get reused).
    let continuing = |pid: u32| -> Option<usize> {
        let &i = index.get(&pid)?;
        (prev.get(&pid)?.starttime == raws[i].starttime).then_some(i)
    };

    let mut next: HashMap<u32, Prev> = raws.iter().map(|r| (r.pid, Prev::of(r))).collect();
    // Zombies keep their last live totals until reaped; never seen alive,
    // there is nothing of theirs to subtract later.
    let mut carried = HashSet::new();
    for &(pid, st) in zombies {
        if let Some(p) = prev.get(&pid).filter(|p| p.starttime == st) {
            next.insert(pid, *p);
            carried.insert(pid);
        }
    }

    // Processes gone since the previous sample. Their last totals were
    // already counted; take them off the cticks/io jump of whoever reaped
    // them: the nearest ancestor that is still around. If that is a zombie,
    // fold them into it so they come off when it is reaped in turn.
    let gone = |pid: u32| !carried.contains(&pid) && continuing(pid).is_none();
    let mut sub_ticks = vec![0u64; raws.len()];
    let mut sub_io = vec![0u64; raws.len()];
    for (&pid, p) in prev {
        if !gone(pid) {
            continue;
        }
        let (ticks, io) = (p.ticks + p.cticks, p.io.unwrap_or(0));
        let mut anc = p.ppid;
        for _ in 0..256 {
            if let Some(i) = continuing(anc) {
                sub_ticks[i] += ticks;
                sub_io[i] += io;
                break;
            }
            if carried.contains(&anc) {
                let z = next.get_mut(&anc).unwrap();
                z.cticks += ticks;
                z.io = z.io.map(|v| v + io);
                break;
            }
            match prev.get(&anc) {
                Some(a) if gone(anc) => anc = a.ppid,
                _ => break,
            }
        }
    }

    let tick = 1.0 / t.clk;
    let mut cpu = Vec::with_capacity(raws.len());
    let mut io = Vec::with_capacity(raws.len());
    for (i, r) in raws.iter().enumerate() {
        let (ticks, cticks, bytes, secs) = match prev.get(&r.pid).filter(|p| p.starttime == r.starttime) {
            Some(p) => (
                r.ticks.saturating_sub(p.ticks),
                r.cticks.saturating_sub(p.cticks),
                match (r.io, p.io) {
                    (Some(a), Some(b)) => a.saturating_sub(b),
                    _ => 0,
                },
                t.dt,
            ),
            None => {
                let start = r.starttime as f64 * tick;
                // Started after the previous sample (give or take a tick);
                // otherwise it was missed, and its totals predate the interval.
                let new = t.prev_uptime.is_some_and(|pu| start + tick >= pu);
                if !new {
                    cpu.push(0.0);
                    io.push(0.0);
                    continue;
                }
                let age = (t.uptime - start).max(tick);
                (r.ticks, r.cticks, r.io.unwrap_or(0), t.dt.min(age))
            }
        };
        let used = ticks + cticks.saturating_sub(sub_ticks[i]);
        let c = used as f64 * tick / secs / t.ncpu * 100.0;
        cpu.push(c.min(100.0) as f32);
        io.push(bytes.saturating_sub(sub_io[i]) as f64 / secs);
    }
    (cpu, io, next)
}

fn read_uptime(buf: &mut Vec<u8>) -> Option<f64> {
    if !read_into(std::path::Path::new("/proc/uptime"), buf) {
        return None;
    }
    std::str::from_utf8(buf).ok()?.split_ascii_whitespace().next()?.parse().ok()
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

    fn raw(pid: u32, ppid: u32, starttime: u64, ticks: u64, cticks: u64, io: u64) -> Raw {
        Raw { pid, ppid, starttime, ticks, cticks, mem: 0, io: Some(io) }
    }

    fn prev_of(raws: &[Raw]) -> HashMap<u32, Prev> {
        raws.iter().map(|r| (r.pid, Prev::of(r))).collect()
    }

    /// clk 100 Hz, one CPU, one second between samples: 1 tick = 1%.
    fn clock(uptime: f64) -> Clock {
        Clock { dt: 1.0, uptime, prev_uptime: Some(uptime - 1.0), clk: 100.0, ncpu: 1.0 }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn exited_child_counted_once() {
        // The child had used 50 ticks (already charged to it) and 1000 bytes
        // of I/O; it ran 30 more ticks and did 500 more bytes before exiting.
        let before = [raw(10, 1, 100, 100, 0, 0), raw(11, 10, 200, 50, 0, 1000)];
        let after = [raw(10, 1, 100, 110, 80, 1500 + 200)];
        let (cpu, io, next) = rates(&prev_of(&before), &after, &[], &clock(500.0));
        assert!(close(cpu[0] as f64, 10.0 + 30.0), "{cpu:?}");
        assert!(close(io[0], 200.0 + 500.0), "{io:?}");
        assert!(!next.contains_key(&11));
    }

    #[test]
    fn new_process_charged_over_its_age() {
        let prev = prev_of(&[raw(1, 0, 1, 0, 0, 0)]);
        // Started 0.5 s ago, at uptime 100.5, with 30 ticks: 0.3 s / 0.5 s.
        let r = [raw(1, 0, 1, 0, 0, 0), raw(42, 1, 10050, 30, 0, 0)];
        let (cpu, _, _) = rates(&prev, &r, &[], &clock(101.0));
        assert!(close(cpu[1] as f64, 30.0 / 100.0 / 0.5 * 100.0), "{cpu:?}");

        // dt shorter than the age (the two clocks drift): divide by dt.
        let t = Clock { dt: 0.4, uptime: 101.0, prev_uptime: Some(100.5), clk: 100.0, ncpu: 1.0 };
        let (cpu, _, _) = rates(&prev, &r, &[], &t);
        assert!(close(cpu[1] as f64, 30.0 / 100.0 / 0.4 * 100.0), "{cpu:?}");

        // Its own children reaped before we saw it count too.
        let r = [raw(42, 1, 10050, 10, 20, 0)];
        let (cpu, _, _) = rates(&prev, &r, &[], &clock(101.0));
        assert!(close(cpu[0] as f64, 60.0), "{cpu:?}");
    }

    #[test]
    fn unseen_old_process_not_charged() {
        // Started long before the previous sample but missing from it (the
        // first sample, or a failed read): its totals aren't from this interval.
        let r = [raw(42, 1, 100, 5000, 0, 0)];
        let (cpu, _, _) = rates(&HashMap::new(), &r, &[], &clock(101.0));
        assert_eq!(cpu, [0.0]);
        let first = Clock { prev_uptime: None, ..clock(101.0) };
        let (cpu, _, _) = rates(&HashMap::new(), &r, &[], &first);
        assert_eq!(cpu, [0.0]);
    }

    #[test]
    fn short_lived_child_charged_to_parent() {
        // A child born and reaped between two samples: 25 ticks, never seen.
        let before = [raw(10, 1, 100, 100, 0, 0)];
        let after = [raw(10, 1, 100, 100, 25, 0)];
        let (cpu, _, _) = rates(&prev_of(&before), &after, &[], &clock(500.0));
        assert!(close(cpu[0] as f64, 25.0), "{cpu:?}");
    }

    #[test]
    fn zombie_charged_when_reaped() {
        let p0 = prev_of(&[raw(10, 1, 100, 100, 0, 0), raw(11, 10, 200, 50, 0, 0)]);
        // The child exits and stays a zombie for a sample: nothing moves yet.
        let (cpu, _, p1) = rates(&p0, &[raw(10, 1, 100, 100, 0, 0)], &[(11, 200)], &clock(500.0));
        assert_eq!(cpu, [0.0]);
        assert_eq!(p1[&11], p0[&11]);
        // Reaped: its whole 70 ticks land in cticks, 50 were already counted.
        let (cpu, _, p2) = rates(&p1, &[raw(10, 1, 100, 100, 70, 0)], &[], &clock(501.0));
        assert!(close(cpu[0] as f64, 20.0), "{cpu:?}");
        assert!(!p2.contains_key(&11));
    }

    #[test]
    fn grandchild_reaped_through_exited_child() {
        // 10 -> 11 -> 12. 12 exits (reaped by 11, 25 ticks in total), then
        // 11 exits (reaped by 10, 15 ticks of its own) in the same interval.
        let before = [raw(10, 1, 100, 0, 0, 0), raw(11, 10, 200, 10, 0, 0), raw(12, 11, 300, 20, 0, 0)];
        let after = [raw(10, 1, 100, 0, 15 + 25, 0)];
        let (cpu, _, _) = rates(&prev_of(&before), &after, &[], &clock(500.0));
        // Already counted: 10 + 20. Left: 5 + 5.
        assert!(close(cpu[0] as f64, 10.0), "{cpu:?}");
    }

    #[test]
    fn reused_pid_is_a_new_process() {
        let before = [raw(10, 1, 100, 0, 0, 0), raw(11, 10, 200, 500, 0, 0)];
        // 11 exited and was reaped (5 ticks after its last sample); a new
        // process got pid 11 at uptime 499.5 and used 10 ticks.
        let after = [raw(10, 1, 100, 0, 505, 0), raw(11, 10, 49950, 10, 0, 0)];
        let (cpu, _, _) = rates(&prev_of(&before), &after, &[], &clock(500.0));
        assert!(close(cpu[0] as f64, 5.0), "{cpu:?}");
        assert!(close(cpu[1] as f64, 20.0), "{cpu:?}");
    }

    #[test]
    fn capped_at_whole_machine() {
        let before = [raw(10, 1, 100, 0, 0, 0)];
        let after = [raw(10, 1, 100, 0, 5000, 0)];
        let (cpu, _, _) = rates(&prev_of(&before), &after, &[], &clock(500.0));
        assert_eq!(cpu, [100.0]);
    }

    #[test]
    fn cgroups() {
        assert_eq!(parse_cgroup(b"0::/user.slice/app.slice/app-steam-1.scope\n"), "/user.slice/app.slice/app-steam-1.scope");
    }
}
