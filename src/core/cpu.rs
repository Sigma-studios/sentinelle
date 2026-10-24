//! CPU usage from `/proc/stat`, frequency from cpufreq, temperature from hwmon.

use super::hwmon::{self, TempSensor};
use super::util::{read_string, read_u64};
use std::collections::HashSet;

#[derive(Clone, Copy, Default)]
struct Times {
    busy: u64,
    total: u64,
}

pub struct CpuSampler {
    pub name: String,
    pub physical_cores: usize,
    prev_total: Times,
    prev_cores: Vec<Times>,
    sensor: Option<TempSensor>,
    /// Sum of the latest total deltas, used by process CPU accounting.
    pub last_total_jiffies: u64,
}

pub struct CpuSample {
    pub usage: f32,
    pub per_core: Vec<f32>,
    pub temp: Option<f32>,
    pub freq_mhz: Option<f32>,
}

impl CpuSampler {
    pub fn new() -> Self {
        let (name, physical_cores) = cpu_identity();
        let mut s = Self {
            name,
            physical_cores,
            prev_total: Times::default(),
            prev_cores: Vec::new(),
            sensor: hwmon::cpu_sensor(),
            last_total_jiffies: 0,
        };
        s.sample();
        s
    }

    pub fn has_temp(&self) -> bool {
        self.sensor.is_some()
    }

    pub fn sample(&mut self) -> CpuSample {
        let stat = read_string("/proc/stat").unwrap_or_default();
        let mut total = Times::default();
        let mut cores = Vec::with_capacity(self.prev_cores.len());
        for line in stat.lines() {
            let Some(rest) = line.strip_prefix("cpu") else { break };
            // "cpu  1 2 3" is the total, "cpu7 1 2 3" a core.
            let is_total = rest.starts_with(' ');
            let mut it = rest.split_ascii_whitespace();
            if !is_total {
                it.next();
            }
            let v: Vec<u64> = it.map(|x| x.parse().unwrap_or(0)).collect();
            if v.len() < 4 {
                continue;
            }
            // user nice system idle iowait irq softirq steal (guest is already in user)
            let idle = v[3] + v.get(4).copied().unwrap_or(0);
            let all: u64 = v.iter().take(8).sum();
            let t = Times { busy: all - idle, total: all };
            if is_total {
                total = t;
            } else {
                cores.push(t);
            }
        }

        let pct = |now: Times, prev: Times| {
            let dt = now.total.saturating_sub(prev.total);
            if dt == 0 {
                0.0
            } else {
                (now.busy.saturating_sub(prev.busy) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0)
            }
        };
        let usage = pct(total, self.prev_total);
        let per_core = cores
            .iter()
            .enumerate()
            .map(|(i, &c)| pct(c, self.prev_cores.get(i).copied().unwrap_or_default()))
            .collect();
        self.last_total_jiffies = total.total.saturating_sub(self.prev_total.total);
        self.prev_total = total;
        self.prev_cores = cores;

        CpuSample {
            usage,
            per_core,
            temp: self.sensor.as_ref().and_then(|s| s.read()),
            freq_mhz: average_freq_mhz(self.prev_cores.len()),
        }
    }
}

fn average_freq_mhz(n: usize) -> Option<f32> {
    let mut sum = 0u64;
    let mut count = 0u64;
    for i in 0..n {
        if let Some(khz) = read_u64(format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq")) {
            sum += khz;
            count += 1;
        }
    }
    (count > 0).then(|| sum as f32 / count as f32 / 1000.0)
}

fn cpu_identity() -> (String, usize) {
    let info = read_string("/proc/cpuinfo").unwrap_or_default();
    let mut name = None;
    let mut cores = HashSet::new();
    let mut physical = "0";
    for line in info.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "model name" | "Model" | "Hardware" if name.is_none() => name = Some(v.to_string()),
            "physical id" => physical = v,
            "core id" => {
                cores.insert(format!("{physical}:{v}"));
            }
            _ => {}
        }
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let physical_cores = if cores.is_empty() { threads } else { cores.len() };
    (clean_cpu_name(&name.unwrap_or_else(|| "Processor".into())), physical_cores)
}

/// "AMD Ryzen 7 5800X 8-Core Processor" -> "AMD Ryzen 7 5800X"
pub fn clean_cpu_name(raw: &str) -> String {
    let mut s = raw.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    for cut in [" with Radeon", " CPU @", " @ "] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    let words: Vec<&str> = s
        .split_whitespace()
        .filter(|w| {
            let lw = w.to_ascii_lowercase();
            !(lw == "processor" || lw == "cpu" || lw.ends_with("-core"))
        })
        .collect();
    if words.is_empty() {
        return "Processor".into();
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::clean_cpu_name;

    #[test]
    fn cpu_names() {
        assert_eq!(clean_cpu_name("AMD Ryzen 7 5800X 8-Core Processor"), "AMD Ryzen 7 5800X");
        assert_eq!(clean_cpu_name("Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz"), "Intel Core i7-8700K");
        assert_eq!(clean_cpu_name("AMD Ryzen 7 7840U w/ Radeon 780M Graphics"), "AMD Ryzen 7 7840U w/ Radeon 780M Graphics");
        assert_eq!(clean_cpu_name("AMD Ryzen 9 7940HS with Radeon 780M Graphics"), "AMD Ryzen 9 7940HS");
    }
}
