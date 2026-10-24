//! Temperature sensors from `/sys/class/hwmon`.

use super::util::{read_i64, read_trimmed};
use std::fs;
use std::path::{Path, PathBuf};

/// A single `tempN_input` file, read in millidegrees.
#[derive(Clone, Debug)]
pub struct TempSensor {
    path: PathBuf,
}

impl TempSensor {
    pub fn read(&self) -> Option<f32> {
        let v = read_i64(&self.path)? as f32 / 1000.0;
        // Some drivers report garbage (e.g. -273 or 0) when a sensor is off.
        (v > -40.0 && v < 150.0 && v != 0.0).then_some(v)
    }
}

/// Labeled temperature inputs of one hwmon directory.
fn temps(dir: &Path) -> Vec<(String, TempSensor)> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if let Some(n) = name.strip_prefix("temp").and_then(|s| s.strip_suffix("_input")) {
            let label = read_trimmed(dir.join(format!("temp{n}_label"))).unwrap_or_else(|| format!("temp{n}"));
            out.push((label, TempSensor { path: e.path() }));
        }
    }
    out.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    out
}

fn devices() -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir("/sys/class/hwmon") else { return Vec::new() };
    let mut v: Vec<_> = rd
        .flatten()
        .filter_map(|e| Some((read_trimmed(e.path().join("name"))?, e.path())))
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// Best guess at the CPU package temperature.
pub fn cpu_sensor() -> Option<TempSensor> {
    let devs = devices();
    // (driver, preferred labels in order)
    const PREFS: &[(&str, &[&str])] = &[
        ("zenpower", &["Tdie", "Tctl"]),
        ("k10temp", &["Tdie", "Tctl"]),
        ("coretemp", &["Package id 0", "Physical id 0"]),
        ("cpu_thermal", &[]),
        ("soc_thermal", &[]),
        ("acpitz", &[]),
    ];
    for (driver, labels) in PREFS {
        for (_, dir) in devs.iter().filter(|(n, _)| n == driver) {
            let t = temps(dir);
            for want in labels.iter() {
                if let Some((_, s)) = t.iter().find(|(l, _)| l == want) {
                    if s.read().is_some() {
                        return Some(s.clone());
                    }
                }
            }
            if let Some((_, s)) = t.into_iter().find(|(_, s)| s.read().is_some()) {
                return Some(s);
            }
        }
    }
    None
}

/// Temperature of a GPU given its PCI device directory.
pub fn gpu_sensor(device_dir: &Path) -> Option<TempSensor> {
    let rd = fs::read_dir(device_dir.join("hwmon")).ok()?;
    for e in rd.flatten() {
        let t = temps(&e.path());
        // amdgpu: edge / junction / mem. Edge matches what most tools show.
        for want in ["edge", "junction", "temp1"] {
            if let Some((_, s)) = t.iter().find(|(l, _)| l == want) {
                return Some(s.clone());
            }
        }
        if let Some((_, s)) = t.into_iter().next() {
            return Some(s);
        }
    }
    None
}

/// First hwmon dir below a PCI device (for power readings).
pub fn device_hwmon(device_dir: &Path) -> Option<PathBuf> {
    fs::read_dir(device_dir.join("hwmon")).ok()?.flatten().map(|e| e.path()).next()
}
