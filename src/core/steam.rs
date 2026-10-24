//! Steam game names from the local library manifests.

use super::util::{home_dir, read_string};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Default)]
pub struct SteamLibrary {
    names: HashMap<u32, String>,
    tried: HashSet<u32>,
}

impl SteamLibrary {
    pub fn name(&mut self, app_id: u32) -> String {
        if !self.names.contains_key(&app_id) && self.tried.insert(app_id) {
            // Rescan once when a game we don't know shows up (freshly installed).
            self.reload();
        }
        self.names.get(&app_id).cloned().unwrap_or_else(|| format!("Steam game {app_id}"))
    }

    fn reload(&mut self) {
        for lib in library_dirs() {
            let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { continue };
            for e in rd.flatten() {
                let fname = e.file_name();
                let fname = fname.to_string_lossy();
                let Some(id) = fname
                    .strip_prefix("appmanifest_")
                    .and_then(|s| s.strip_suffix(".acf"))
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                if self.names.contains_key(&id) {
                    continue;
                }
                if let Some(name) = read_string(e.path()).as_deref().and_then(|t| vdf_value(t, "name")) {
                    self.names.insert(id, name);
                }
            }
        }
    }
}

fn steam_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else { return Vec::new() };
    [
        ".local/share/Steam",
        ".steam/steam",
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        "snap/steam/common/.local/share/Steam",
    ]
    .iter()
    .map(|p| home.join(p))
    .filter(|p| p.join("steamapps").is_dir())
    .collect()
}

fn library_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in steam_roots() {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        if !out.contains(&root) {
            out.push(root.clone());
        }
        let Some(vdf) = read_string(root.join("steamapps/libraryfolders.vdf")) else { continue };
        for line in vdf.lines() {
            let mut parts = quoted(line);
            if parts.next().as_deref() == Some("path") {
                if let Some(p) = parts.next() {
                    let p = PathBuf::from(p);
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// Quoted strings in a line of Valve's KeyValues format.
fn quoted(line: &str) -> impl Iterator<Item = String> + '_ {
    line.split('"').skip(1).step_by(2).map(|s| s.replace("\\\\", "\\"))
}

fn vdf_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut q = quoted(l);
        (q.next().as_deref() == Some(key)).then(|| q.next()).flatten()
    })
}

/// App id from a process's command line, as launched by Steam's `reaper`:
/// `reaper SteamLaunch AppId=1245620 -- ...`.
pub fn app_id_from_cmdline(args: &[&str]) -> Option<u32> {
    if !args.iter().take(4).any(|a| *a == "SteamLaunch") {
        return None;
    }
    args.iter().find_map(|a| a.strip_prefix("AppId=")?.parse().ok()).filter(|&id| id != 0)
}

/// App id from a process environment block (NUL separated).
pub fn app_id_from_environ(env: &[u8]) -> Option<u32> {
    for var in env.split(|&b| b == 0) {
        for key in [&b"SteamGameId="[..], b"SteamAppId="] {
            if let Some(v) = var.strip_prefix(key) {
                if let Some(id) = std::str::from_utf8(v).ok().and_then(|s| s.parse::<u64>().ok()) {
                    // Non-Steam shortcuts get 64-bit "game ids"; keep the low bits.
                    let id = if id > u64::from(u32::MAX) { (id >> 32) as u32 } else { id as u32 };
                    if id != 0 {
                        return Some(id);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reaper() {
        let args = ["/home/u/.steam/ubuntu12_32/reaper", "SteamLaunch", "AppId=1245620", "--", "/x/proton"];
        assert_eq!(app_id_from_cmdline(&args), Some(1245620));
        assert_eq!(app_id_from_cmdline(&["bash", "AppId=3"]), None);
    }

    #[test]
    fn parses_env() {
        assert_eq!(app_id_from_environ(b"A=1\0SteamAppId=0\0SteamGameId=570\0"), Some(570));
        assert_eq!(app_id_from_environ(b"A=1\0"), None);
    }

    #[test]
    fn parses_manifest() {
        let t = "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n}";
        assert_eq!(vdf_value(t, "name").as_deref(), Some("Dota 2"));
    }
}
