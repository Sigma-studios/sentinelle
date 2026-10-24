//! GPUs: amdgpu / i915 / xe through sysfs, NVIDIA through NVML.

use super::hwmon::{self, TempSensor};
use super::nvml::Nvml;
use super::util::{read_string, read_trimmed, read_u64};
use super::GpuVendor;
use std::fs;
use std::path::{Path, PathBuf};

pub struct GpuSample {
    pub usage: Option<f32>,
    pub temp: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub power_w: Option<f32>,
    pub clock_mhz: Option<u32>,
}

enum Source {
    Amd { dev: PathBuf, hwmon: Option<PathBuf> },
    Intel { dev: PathBuf, card: PathBuf },
    Nvidia { index: usize },
}

pub struct Gpu {
    pub name: String,
    pub vendor: GpuVendor,
    pub usage_is_clock: bool,
    temp: Option<TempSensor>,
    source: Source,
}

pub struct GpuSampler {
    pub gpus: Vec<Gpu>,
    nvml: Option<Nvml>,
}

impl GpuSampler {
    pub fn new() -> Self {
        let mut gpus = Vec::new();
        let mut cards: Vec<PathBuf> = fs::read_dir("/sys/class/drm")
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        let n = p.file_name().unwrap().to_string_lossy();
                        n.starts_with("card") && n[4..].chars().all(|c| c.is_ascii_digit())
                    })
                    .collect()
            })
            .unwrap_or_default();
        cards.sort();

        let mut has_nvidia = false;
        let mut seen = Vec::new();
        for card in cards {
            let dev = card.join("device");
            let Ok(real) = fs::canonicalize(&dev) else { continue };
            if seen.contains(&real) {
                continue;
            }
            seen.push(real);
            let vendor = read_trimmed(dev.join("vendor")).unwrap_or_default();
            let device = read_trimmed(dev.join("device")).unwrap_or_default();
            let name = pci_name(&vendor, &device);
            match vendor.as_str() {
                "0x1002" => gpus.push(Gpu {
                    name: name.unwrap_or_else(|| "Radeon Graphics".into()),
                    vendor: GpuVendor::Amd,
                    usage_is_clock: false,
                    temp: hwmon::gpu_sensor(&dev),
                    source: Source::Amd { hwmon: hwmon::device_hwmon(&dev), dev },
                }),
                "0x8086" => gpus.push(Gpu {
                    name: name.unwrap_or_else(|| "Intel Graphics".into()),
                    vendor: GpuVendor::Intel,
                    usage_is_clock: true,
                    temp: hwmon::gpu_sensor(&dev),
                    source: Source::Intel { dev, card },
                }),
                "0x10de" => has_nvidia = true,
                _ => {}
            }
        }

        let nvml = if has_nvidia { Nvml::load() } else { None };
        if let Some(n) = &nvml {
            for index in 0..n.count() {
                gpus.push(Gpu {
                    name: n.name(index),
                    vendor: GpuVendor::Nvidia,
                    usage_is_clock: false,
                    temp: None,
                    source: Source::Nvidia { index },
                });
            }
        }
        // Discrete cards first: they're the interesting ones.
        gpus.sort_by_key(|g| matches!(g.vendor, GpuVendor::Intel));
        Self { gpus, nvml }
    }

    pub fn sample(&self) -> Vec<GpuSample> {
        self.gpus.iter().map(|g| self.sample_one(g)).collect()
    }

    fn sample_one(&self, g: &Gpu) -> GpuSample {
        let temp = g.temp.as_ref().and_then(|t| t.read());
        match &g.source {
            Source::Amd { dev, hwmon } => {
                let power = hwmon.as_ref().and_then(|h| {
                    read_u64(h.join("power1_average")).or_else(|| read_u64(h.join("power1_input")))
                });
                let clock = hwmon.as_ref().and_then(|h| read_u64(h.join("freq1_input")));
                GpuSample {
                    usage: read_u64(dev.join("gpu_busy_percent")).map(|v| v as f32),
                    temp,
                    vram_used: read_u64(dev.join("mem_info_vram_used")),
                    vram_total: read_u64(dev.join("mem_info_vram_total")),
                    power_w: power.map(|uw| uw as f32 / 1e6),
                    clock_mhz: clock.map(|hz| (hz / 1_000_000) as u32),
                }
            }
            Source::Intel { dev, card } => {
                let (cur, max) = intel_freq(card, dev);
                GpuSample {
                    usage: match (cur, max) {
                        (Some(c), Some(m)) if m > 0 => Some((c as f32 / m as f32 * 100.0).min(100.0)),
                        _ => None,
                    },
                    temp,
                    vram_used: None,
                    vram_total: None,
                    power_w: None,
                    clock_mhz: cur,
                }
            }
            Source::Nvidia { index } => {
                let s = self.nvml.as_ref().unwrap().sample(*index);
                GpuSample {
                    usage: s.usage,
                    temp: s.temp,
                    vram_used: s.vram_used,
                    vram_total: s.vram_total,
                    power_w: s.power_w,
                    clock_mhz: s.clock_mhz,
                }
            }
        }
    }
}

fn intel_freq(card: &Path, dev: &Path) -> (Option<u32>, Option<u32>) {
    // i915
    if let Some(cur) = read_u64(card.join("gt_act_freq_mhz")) {
        let max = read_u64(card.join("gt_RP0_freq_mhz")).or_else(|| read_u64(card.join("gt_max_freq_mhz")));
        return (Some(cur as u32), max.map(|m| m as u32));
    }
    // xe
    let base = dev.join("tile0/gt0/freq0");
    let cur = read_u64(base.join("act_freq"));
    let max = read_u64(base.join("rp0_freq")).or_else(|| read_u64(base.join("max_freq")));
    (cur.map(|c| c as u32), max.map(|m| m as u32))
}

/// Marketing name from the system's pci.ids, e.g.
/// "Navi 31 [Radeon RX 7900 XT/7900 XTX]" -> "Radeon RX 7900 XT/7900 XTX".
fn pci_name(vendor: &str, device: &str) -> Option<String> {
    let vendor = vendor.trim_start_matches("0x").to_ascii_lowercase();
    let device = device.trim_start_matches("0x").to_ascii_lowercase();
    let db = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"]
        .iter()
        .find_map(read_string)?;
    let mut in_vendor = false;
    for line in db.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                break;
            }
            in_vendor = line.starts_with(&vendor) && line.as_bytes().get(4) == Some(&b' ');
            continue;
        }
        if in_vendor && !line.starts_with("\t\t") {
            let l = &line[1..];
            if l.starts_with(&device) {
                return Some(prettify_pci_name(l[4..].trim()));
            }
        }
    }
    None
}

fn prettify_pci_name(raw: &str) -> String {
    match (raw.find('['), raw.rfind(']')) {
        (Some(a), Some(b)) if b > a + 1 => raw[a + 1..b].to_string(),
        _ => raw.to_string(),
    }
}
