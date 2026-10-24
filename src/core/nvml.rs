//! Minimal NVML binding, loaded at runtime so the binary has no hard
//! dependency on the NVIDIA driver.

use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

type Device = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

pub struct Nvml {
    lib: Library,
    devices: Vec<Device>,
}

// NVML handles are thread-safe per NVIDIA's documentation.
unsafe impl Send for Nvml {}

pub struct NvmlSample {
    pub usage: Option<f32>,
    pub temp: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub power_w: Option<f32>,
    pub clock_mhz: Option<u32>,
}

impl Nvml {
    pub fn load() -> Option<Self> {
        unsafe {
            let lib = Library::new("libnvidia-ml.so.1").or_else(|_| Library::new("libnvidia-ml.so")).ok()?;
            let init: Symbol<unsafe extern "C" fn() -> c_int> = lib.get(b"nvmlInit_v2\0").ok()?;
            if init() != 0 {
                return None;
            }
            let count_fn: Symbol<unsafe extern "C" fn(*mut c_uint) -> c_int> = lib.get(b"nvmlDeviceGetCount_v2\0").ok()?;
            let handle_fn: Symbol<unsafe extern "C" fn(c_uint, *mut Device) -> c_int> =
                lib.get(b"nvmlDeviceGetHandleByIndex_v2\0").ok()?;
            let mut count = 0;
            if count_fn(&mut count) != 0 {
                return None;
            }
            let mut devices = Vec::new();
            for i in 0..count {
                let mut d: Device = std::ptr::null_mut();
                if handle_fn(i, &mut d) == 0 {
                    devices.push(d);
                }
            }
            Some(Self { lib, devices })
        }
    }

    pub fn count(&self) -> usize {
        self.devices.len()
    }

    pub fn name(&self, i: usize) -> String {
        unsafe {
            let f: Result<Symbol<unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int>, _> =
                self.lib.get(b"nvmlDeviceGetName\0");
            let mut buf = [0 as c_char; 96];
            if let Ok(f) = f {
                if f(self.devices[i], buf.as_mut_ptr(), buf.len() as c_uint) == 0 {
                    let name = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
                    return name.trim_start_matches("NVIDIA ").to_string();
                }
            }
            "GPU".into()
        }
    }

    pub fn sample(&self, i: usize) -> NvmlSample {
        let d = self.devices[i];
        unsafe {
            let util = self.call::<Utilization>(b"nvmlDeviceGetUtilizationRates\0", d);
            let mem = self.call::<Memory>(b"nvmlDeviceGetMemoryInfo\0", d);
            let power = self.call::<c_uint>(b"nvmlDeviceGetPowerUsage\0", d);
            let temp = self.call_arg::<c_uint>(b"nvmlDeviceGetTemperature\0", d, 0);
            let clock = self.call_arg::<c_uint>(b"nvmlDeviceGetClockInfo\0", d, 0);
            NvmlSample {
                usage: util.map(|u| u.gpu as f32),
                temp: temp.map(|t| t as f32),
                vram_used: mem.as_ref().map(|m| m.used),
                vram_total: mem.as_ref().map(|m| m.total),
                power_w: power.map(|p| p as f32 / 1000.0),
                clock_mhz: clock,
            }
        }
    }

    unsafe fn call<T: Default>(&self, sym: &[u8], d: Device) -> Option<T> {
        let f: Symbol<unsafe extern "C" fn(Device, *mut T) -> c_int> = self.lib.get(sym).ok()?;
        let mut out = T::default();
        (f(d, &mut out) == 0).then_some(out)
    }

    unsafe fn call_arg<T: Default>(&self, sym: &[u8], d: Device, arg: c_int) -> Option<T> {
        let f: Symbol<unsafe extern "C" fn(Device, c_int, *mut T) -> c_int> = self.lib.get(sym).ok()?;
        let mut out = T::default();
        (f(d, arg, &mut out) == 0).then_some(out)
    }
}
