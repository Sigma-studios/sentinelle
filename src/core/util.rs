use std::fs;
use std::path::Path;

pub fn read_string(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

pub fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    read_string(path).map(|s| s.trim().to_string())
}

pub fn read_u64(path: impl AsRef<Path>) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

pub fn read_i64(path: impl AsRef<Path>) -> Option<i64> {
    read_trimmed(path)?.parse().ok()
}

/// Reads a file into a reusable buffer; returns false on error.
pub fn read_into(path: &Path, buf: &mut Vec<u8>) -> bool {
    use std::io::Read;
    buf.clear();
    match fs::File::open(path) {
        Ok(mut f) => f.read_to_end(buf).is_ok(),
        Err(_) => false,
    }
}

pub fn clk_tck() -> f64 {
    #[cfg(unix)]
    {
        let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if v > 0 {
            return v as f64;
        }
    }
    100.0
}

pub fn page_size() -> u64 {
    #[cfg(unix)]
    {
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 {
            return v as u64;
        }
    }
    4096
}

pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Lower the calling thread's CPU and IO priority, for background work.
pub fn deprioritize_current_thread() {
    #[cfg(target_os = "linux")]
    unsafe {
        // On Linux, PRIO_PROCESS with id 0 applies to the calling thread.
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
        // ioprio_set(IOPRIO_WHO_PROCESS, 0, IOPRIO_CLASS_IDLE << 13)
        libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13);
    }
}

pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(Into::into)
}
