use super::util::read_string;

#[derive(Default)]
pub struct MemSample {
    pub total: u64,
    pub used: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

pub fn sample() -> MemSample {
    let text = read_string("/proc/meminfo").unwrap_or_default();
    let get = |key: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let total = get("MemTotal");
    let available = get("MemAvailable");
    let cached = get("Cached") + get("Buffers");
    let swap_total = get("SwapTotal");
    let swap_free = get("SwapFree");
    MemSample {
        total,
        used: total.saturating_sub(available),
        cached,
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
    }
}
