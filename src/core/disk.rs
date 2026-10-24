//! Mounted filesystems and a background scan of where their space goes.
//!
//! The scan builds a pruned size tree (only directories big enough to ever
//! matter are kept) and then [`partition`] picks a handful of paths so each
//! one is roughly between 10% and 50% of the used space.

use super::util::{deprioritize_current_thread, home_dir, read_string};
use super::{DiskSegment, SegmentKind};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const REAL_FS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "zfs", "bcachefs", "jfs", "reiserfs", "ntfs", "ntfs3", "fuseblk",
    "exfat", "vfat", "nilfs2",
];

#[derive(Clone, Debug)]
pub struct Filesystem {
    pub id: String,
    pub device: String,
    pub fs_type: String,
    pub mount_points: Vec<PathBuf>,
}

fn unescape_mount(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// All mount points (for not crossing into nested mounts) and the real
/// filesystems worth showing, grouped by backing device.
pub fn filesystems() -> (Vec<Filesystem>, HashSet<PathBuf>) {
    let text = read_string("/proc/self/mountinfo").unwrap_or_default();
    let mut all = HashSet::new();
    let mut by_dev: Vec<Filesystem> = Vec::new();
    let mut seen_roots = HashSet::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else { continue };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 5 || r.len() < 2 {
            continue;
        }
        let mount = PathBuf::from(unescape_mount(l[4]));
        all.insert(mount.clone());
        let (fs_type, source) = (r[0], unescape_mount(r[1]));
        if !REAL_FS.contains(&fs_type) {
            continue;
        }
        let m = mount.to_string_lossy();
        if m.starts_with("/snap/") || m.starts_with("/var/lib/docker") || m.starts_with("/var/lib/containers")
            || m.starts_with("/run/") || m.starts_with("/proc") || m.starts_with("/sys")
        {
            continue;
        }
        if !mount.is_dir() {
            continue; // file bind mounts (containers)
        }
        // Bind mounts of the same directory: keep the first.
        if !seen_roots.insert((l[2].to_string(), l[3].to_string())) {
            continue;
        }
        if let Some(fs) = by_dev.iter_mut().find(|f| f.id == source) {
            fs.mount_points.push(mount);
        } else {
            by_dev.push(Filesystem {
                id: source.clone(),
                device: source.rsplit('/').next().unwrap_or(&source).to_string(),
                fs_type: fs_type.to_string(),
                mount_points: vec![mount],
            });
        }
    }
    for fs in &mut by_dev {
        fs.mount_points.sort_by_key(|p| p.as_os_str().len());
    }
    // Hide tiny boot partitions and the like.
    by_dev.retain(|fs| space(&fs.mount_points[0]).is_some_and(|(total, _)| total >= 2 << 30));
    by_dev.sort_by_key(|fs| {
        let p = &fs.mount_points[0];
        (p != Path::new("/"), !p.starts_with("/home"), p.clone())
    });
    (by_dev, all)
}

/// (total, used) bytes.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        let bs = s.f_frsize as u64;
        let total = s.f_blocks as u64 * bs;
        // Used as `df` shows it: excludes root-reserved blocks from free.
        let used = (s.f_blocks as u64 - s.f_bfree as u64) * bs;
        Some((total, used))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

// ---------------------------------------------------------------- scanning

#[derive(Debug, Default, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub size: u64,
    /// Bytes directly in this directory plus pruned children.
    pub own: u64,
    pub children: Vec<Node>,
}

struct Walk<'a> {
    dev: u64,
    min_keep: u64,
    skip: &'a HashSet<PathBuf>,
    inodes: Mutex<HashSet<u64>>,
    progress: &'a AtomicU64,
}

impl Walk<'_> {
    fn dir(&self, path: PathBuf) -> Node {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let mut node = Node { path, ..Default::default() };
        let Ok(rd) = std::fs::read_dir(&node.path) else { return node };
        let mut subdirs = Vec::new();
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                subdirs.push(e.path());
                continue;
            }
            #[cfg(unix)]
            if let Ok(md) = e.metadata() {
                let bytes = md.blocks() * 512;
                if md.nlink() > 1 && !ft.is_symlink() && !self.inodes.lock().unwrap().insert(md.ino()) {
                    continue; // hard link already counted
                }
                node.own += bytes;
                self.progress.fetch_add(bytes, Ordering::Relaxed);
            }
        }
        let children: Vec<Node> = subdirs
            .into_par_iter()
            .filter(|p| !self.skip.contains(p) && same_dev(p, self.dev))
            .map(|p| self.dir(p))
            .collect();
        node.size = node.own;
        for c in children {
            node.size += c.size;
            if c.size >= self.min_keep {
                node.children.push(c);
            } else {
                node.own += c.size;
            }
        }
        node
    }
}

fn same_dev(p: &Path, dev: u64) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(p).map(|m| m.dev() == dev).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = (p, dev);
        true
    }
}

/// Scan every mount point of a filesystem. Returns a virtual root whose
/// children are the mount points.
pub fn scan(fs: &Filesystem, all_mounts: &HashSet<PathBuf>, used: u64, progress: &AtomicU64) -> Node {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let mut root = Node::default();
    for mp in &fs.mount_points {
        #[cfg(unix)]
        let dev = std::fs::metadata(mp).map(|m| m.dev()).unwrap_or(0);
        #[cfg(not(unix))]
        let dev = 0;
        let mut skip = all_mounts.clone();
        skip.remove(mp);
        let walk = Walk {
            dev,
            min_keep: used / 200, // 0.5%
            skip: &skip,
            inodes: Mutex::new(HashSet::new()),
            progress,
        };
        let n = walk.dir(mp.clone());
        root.size += n.size;
        root.children.push(n);
    }
    root
}

/// Choose paths so each covers roughly 10%..50% of `total`.
pub fn partition(root: &Node, total: u64) -> Vec<DiskSegment> {
    let total = total.max(1) as f64;
    let hi = (0.5 * total) as u64;
    let lo = (0.1 * total) as u64;
    let floor = (0.02 * total) as u64;
    let mut out = Vec::new();
    let mut other = 0u64;
    let mut work: Vec<&Node> = vec![root];

    while let Some(n) = work.pop() {
        let is_virtual = n.path.as_os_str().is_empty();
        if (n.size > hi || is_virtual) && !n.children.is_empty() {
            let mut kids: Vec<&Node> = n.children.iter().collect();
            kids.sort_by_key(|c| std::cmp::Reverse(c.size));
            let mut rest = n.size;
            for c in kids {
                // Take everything >= 10%; dig a little deeper (down to 2%)
                // while what's left behind is still too big to be useful.
                if c.size >= lo || (rest > hi && c.size >= floor) || (is_virtual && c.size >= floor) {
                    rest -= c.size;
                    work.push(c);
                }
            }
            if !is_virtual && rest >= lo {
                out.push(DiskSegment { path: n.path.clone(), size: rest, kind: SegmentKind::Rest });
            } else {
                other += rest;
            }
        } else if n.size >= floor {
            out.push(DiskSegment { path: n.path.clone(), size: n.size, kind: SegmentKind::Dir });
        } else {
            other += n.size;
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.size));
    if other > 0 {
        out.push(DiskSegment { path: PathBuf::new(), size: other, kind: SegmentKind::Other });
    }
    out
}

// ---------------------------------------------------------------- service

pub struct ScanResult {
    pub segments: Vec<DiskSegment>,
    pub at: SystemTime,
}

#[derive(Default)]
pub struct DiskState {
    pub results: HashMap<String, ScanResult>,
    /// Filesystem id currently being scanned and its progress.
    pub scanning: Option<(String, Arc<AtomicU64>)>,
}

pub type SharedDisks = Arc<Mutex<DiskState>>;

const RESCAN_EVERY: Duration = Duration::from_secs(20 * 60);

/// Scans all filesystems now and then every [`RESCAN_EVERY`], or when
/// something is sent on the returned channel.
pub fn spawn_scanner(state: SharedDisks) -> std::sync::mpsc::Sender<()> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    for (id, r) in load_cache() {
        state.lock().unwrap().results.insert(id, r);
    }
    std::thread::Builder::new()
        .name("disk-scan".into())
        .spawn(move || {
            deprioritize_current_thread();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(4)
                .thread_name(|i| format!("disk-scan-{i}"))
                .start_handler(|_| deprioritize_current_thread())
                .build()
                .expect("thread pool");
            let mut first = true;
            loop {
                let (filesystems, mounts) = filesystems();
                for fs in &filesystems {
                    let fresh = state
                        .lock()
                        .unwrap()
                        .results
                        .get(&fs.id)
                        .and_then(|r| r.at.elapsed().ok())
                        .is_some_and(|age| age < RESCAN_EVERY);
                    if first && fresh {
                        continue; // cache is recent enough
                    }
                    let Some((_, used)) = space(&fs.mount_points[0]) else { continue };
                    let progress = Arc::new(AtomicU64::new(0));
                    state.lock().unwrap().scanning = Some((fs.id.clone(), progress.clone()));
                    let tree = pool.install(|| scan(fs, &mounts, used, &progress));
                    let segments = partition(&tree, tree.size);
                    let mut st = state.lock().unwrap();
                    st.scanning = None;
                    st.results.insert(fs.id.clone(), ScanResult { segments, at: SystemTime::now() });
                    save_cache(&st.results);
                }
                first = false;
                match rx.recv_timeout(RESCAN_EVERY) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        while rx.try_recv().is_ok() {}
                    }
                    Err(_) => return,
                }
            }
        })
        .expect("spawn disk scanner");
    tx
}

fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home_dir().map(|h| h.join(".cache")))?;
    Some(base.join("sentinelle").join("disks.tsv"))
}

fn save_cache(results: &HashMap<String, ScanResult>) {
    let Some(path) = cache_path() else { return };
    let mut s = String::new();
    for (id, r) in results {
        let at = r.at.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        for seg in &r.segments {
            let kind = match seg.kind {
                SegmentKind::Dir => "dir",
                SegmentKind::Rest => "rest",
                SegmentKind::Other => "other",
                SegmentKind::Hidden => continue,
            };
            s.push_str(&format!("{id}\t{at}\t{kind}\t{}\t{}\n", seg.size, seg.path.display()));
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, s);
}

fn load_cache() -> HashMap<String, ScanResult> {
    let mut out: HashMap<String, ScanResult> = HashMap::new();
    let Some(text) = cache_path().and_then(read_string) else { return out };
    for line in text.lines() {
        let f: Vec<&str> = line.splitn(5, '\t').collect();
        if f.len() != 5 {
            continue;
        }
        let kind = match f[2] {
            "dir" => SegmentKind::Dir,
            "rest" => SegmentKind::Rest,
            _ => SegmentKind::Other,
        };
        let (Ok(at), Ok(size)) = (f[1].parse::<u64>(), f[3].parse::<u64>()) else { continue };
        let r = out.entry(f[0].to_string()).or_insert_with(|| ScanResult {
            segments: Vec::new(),
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(at),
        });
        r.segments.push(DiskSegment { path: PathBuf::from(f[4]), size, kind });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(path: &str, own: u64, children: Vec<Node>) -> Node {
        let size = own + children.iter().map(|c| c.size).sum::<u64>();
        Node { path: path.into(), size, own, children }
    }

    #[test]
    fn splits_until_under_half() {
        // / = 100: /home 70 (/home/u 68: Games 40, .cache 15, rest 13), /usr 25, small 5
        let tree = n(
            "",
            0,
            vec![n(
                "/",
                5,
                vec![
                    n("/home", 2, vec![n("/home/u", 13, vec![n("/home/u/Games", 40, vec![]), n("/home/u/.cache", 15, vec![])])]),
                    n("/usr", 25, vec![]),
                ],
            )],
        );
        let segs = partition(&tree, tree.size);
        let names: Vec<_> = segs.iter().map(|s| (s.path.to_string_lossy().into_owned(), s.size)).collect();
        assert_eq!(
            names,
            vec![
                ("/home/u/Games".into(), 40),
                ("/usr".into(), 25),
                ("/home/u/.cache".into(), 15),
                ("/home/u".into(), 13),
                ("".into(), 7),
            ]
        );
        assert!(segs.iter().all(|s| s.size <= 50));
    }

    #[test]
    fn flat_directory_digs_to_two_percent() {
        let kids: Vec<Node> = (0..20).map(|i| n(&format!("/data/d{i}"), 5, vec![])).collect();
        let tree = n("", 0, vec![n("/data", 0, kids)]);
        let segs = partition(&tree, tree.size);
        let rest: u64 = segs.iter().filter(|s| s.kind != SegmentKind::Dir).map(|s| s.size).sum();
        assert!(rest <= 50, "rest {rest}");
    }
}
