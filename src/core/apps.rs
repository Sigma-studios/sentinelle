//! Human readable application names from `.desktop` files, and app ids
//! from systemd cgroup names.

use super::util::{home_dir, read_string};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default)]
pub struct DesktopNames {
    /// Lowercased desktop id (file stem) -> Name.
    by_id: HashMap<String, String>,
    /// Basename of the Exec binary -> Name.
    by_exec: HashMap<String, String>,
    loaded: bool,
}

const GENERIC_EXECS: &[&str] = &[
    "sh", "bash", "env", "python", "python3", "java", "flatpak", "snap", "wine", "gtk-launch", "kioclient",
    "kioclient5", "xdg-open", "steam-runtime", "sudo", "pkexec", "electron", "node", "perl", "dbus-send",
];

impl DesktopNames {
    fn load(&mut self) {
        self.loaded = true;
        let mut dirs: Vec<PathBuf> = Vec::new();
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local/share")));
        if let Some(h) = &data_home {
            dirs.push(h.join("applications"));
            dirs.push(h.join("flatpak/exports/share/applications"));
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        dirs.extend(data_dirs.split(':').filter(|s| !s.is_empty()).map(|d| PathBuf::from(d).join("applications")));
        dirs.push("/var/lib/flatpak/exports/share/applications".into());
        dirs.push("/var/lib/snapd/desktop/applications".into());

        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("desktop") {
                    continue;
                }
                let id = p.file_stem().unwrap().to_string_lossy().to_lowercase();
                if self.by_id.contains_key(&id) {
                    continue; // earlier dirs take precedence
                }
                let Some(text) = read_string(&p) else { continue };
                let (name, exec) = parse_entry(&text);
                let Some(name) = name else { continue };
                if let Some(exec) = exec {
                    if !GENERIC_EXECS.contains(&exec.as_str()) {
                        self.by_exec.entry(exec).or_insert_with(|| name.clone());
                    }
                }
                self.by_id.insert(id, name);
            }
        }
    }

    pub fn by_app_id(&mut self, app_id: &str) -> Option<String> {
        if !self.loaded {
            self.load();
        }
        let id = app_id.to_lowercase();
        self.by_id.get(&id).cloned().or_else(|| {
            // Snap ids look like "firefox_firefox".
            let short = id.rsplit('.').next()?;
            self.by_id.get(short).cloned()
        })
    }

    pub fn by_exec(&mut self, exe: &str) -> Option<String> {
        if !self.loaded {
            self.load();
        }
        self.by_exec.get(exe).or_else(|| self.by_id.get(&exe.to_lowercase())).cloned()
    }
}

fn parse_entry(text: &str) -> (Option<String>, Option<String>) {
    let mut in_main = false;
    let mut name = None;
    let mut exec = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_main {
                break;
            }
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec = v
                .split_whitespace()
                .find(|t| !t.contains('=') && *t != "env")
                .map(|t| t.trim_matches('"').rsplit('/').next().unwrap_or(t).to_string());
        } else if line == "NoDisplay=true" || line == "Hidden=true" {
            // Still useful for naming helpers; keep it.
        }
    }
    (name, exec)
}

/// Extract the application id from a systemd unit following the XDG
/// convention: `app[-<launcher>]-<id>[@<rand>].service` or
/// `app[-<launcher>]-<id>-<rand>.scope`. Also handles snap scopes.
pub fn app_id_from_unit(unit: &str) -> Option<String> {
    let unit = unescape(unit);
    if let Some(rest) = unit.strip_prefix("snap.") {
        return rest.split('.').next().map(str::to_string);
    }
    let rest = unit.strip_prefix("app-")?;
    let (body, is_scope) = if let Some(b) = rest.strip_suffix(".scope") {
        (b, true)
    } else {
        (rest.strip_suffix(".service")?, false)
    };
    let mut body = body.split('@').next()?.to_string();
    if is_scope || body.contains('-') {
        // Drop a trailing random/pid suffix.
        if let Some((head, tail)) = body.rsplit_once('-') {
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_hexdigit()) && tail.len() >= 3 {
                body = head.to_string();
            }
        }
    }
    for launcher in ["flatpak-", "gnome-", "kde-", "dbus-:1.", "niri-", "cosmic-", "hyprland-", "sway-"] {
        if let Some(b) = body.strip_prefix(launcher) {
            if launcher == "dbus-:1." {
                return None;
            }
            body = b.to_string();
            break;
        }
    }
    (!body.is_empty()).then_some(body)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&s[i + 2..i + 4], 16) {
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

/// "org.kde.dolphin" -> "Dolphin", "google-chrome" -> "Google chrome".
pub fn prettify_id(id: &str) -> String {
    let last = if id.matches('.').count() >= 2 { id.rsplit('.').next().unwrap_or(id) } else { id };
    let mut s = last.replace(['-', '_'], " ");
    if let Some(f) = s.get(0..1) {
        let up = f.to_uppercase();
        s.replace_range(0..1, &up);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(app_id_from_unit("app-org.kde.konsole-1a2b3c4d.scope").as_deref(), Some("org.kde.konsole"));
        assert_eq!(app_id_from_unit("app-org.kde.dolphin@3f2e1d.service").as_deref(), Some("org.kde.dolphin"));
        assert_eq!(app_id_from_unit("app-flatpak-com.spotify.Client-12345.scope").as_deref(), Some("com.spotify.Client"));
        assert_eq!(app_id_from_unit("app-gnome-firefox-4321.scope").as_deref(), Some("firefox"));
        assert_eq!(app_id_from_unit("app-steam-9876.scope").as_deref(), Some("steam"));
        assert_eq!(app_id_from_unit("app-google\\x2dchrome-9876.scope").as_deref(), Some("google-chrome"));
        assert_eq!(app_id_from_unit("snap.firefox.firefox-abcd.scope").as_deref(), Some("firefox"));
        assert_eq!(app_id_from_unit("pipewire.service"), None);
        assert_eq!(app_id_from_unit("session-2.scope"), None);
    }

    #[test]
    fn pretty() {
        assert_eq!(prettify_id("org.kde.dolphin"), "Dolphin");
        assert_eq!(prettify_id("steam"), "Steam");
    }
}
