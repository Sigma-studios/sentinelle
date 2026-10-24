//! Latency probe. Uses unprivileged ICMP ("ping sockets", allowed by
//! `net.ipv4.ping_group_range` on most distributions) and falls back to
//! timing a TCP handshake when that's not available.

use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub enum Probe {
    Ok(f32),
    Lost,
}

pub struct Pinger {
    host: String,
    addr: Option<IpAddr>,
    resolved_at: Option<Instant>,
    seq: u16,
    pub method: &'static str,
    icmp_broken: bool,
}

impl Pinger {
    pub fn new(host: String) -> Self {
        Self { host, addr: None, resolved_at: None, seq: 0, method: "icmp", icmp_broken: false }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    fn resolve(&mut self) -> Option<IpAddr> {
        let stale = self.resolved_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300));
        if self.addr.is_none() || stale {
            self.resolved_at = Some(Instant::now());
            let addrs: Vec<SocketAddr> = (self.host.as_str(), 0).to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
            self.addr = addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).map(|a| a.ip());
        }
        self.addr
    }

    pub fn probe(&mut self, timeout: Duration) -> Probe {
        let Some(addr) = self.resolve() else { return Probe::Lost };
        self.seq = self.seq.wrapping_add(1);
        if !self.icmp_broken {
            if let IpAddr::V4(v4) = addr {
                match icmp_v4(v4, self.seq, timeout) {
                    Ok(Some(ms)) => {
                        self.method = "icmp";
                        return Probe::Ok(ms);
                    }
                    Ok(None) => return Probe::Lost,
                    Err(_) => self.icmp_broken = true,
                }
            }
        }
        self.method = "tcp";
        let start = Instant::now();
        for port in [443, 80, 53] {
            match TcpStream::connect_timeout(&SocketAddr::new(addr, port), timeout) {
                Ok(_) => return Probe::Ok(start.elapsed().as_secs_f32() * 1000.0),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    // A refusal is still a full round trip.
                    return Probe::Ok(start.elapsed().as_secs_f32() * 1000.0);
                }
                Err(_) if start.elapsed() >= timeout => break,
                Err(_) => {}
            }
        }
        Probe::Lost
    }
}

/// Ok(Some(ms)) on reply, Ok(None) on timeout, Err if ping sockets are unusable.
#[cfg(target_os = "linux")]
fn icmp_v4(addr: std::net::Ipv4Addr, seq: u16, timeout: Duration) -> std::io::Result<Option<f32>> {
    use std::io::Error;
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::IPPROTO_ICMP);
        if fd < 0 {
            return Err(Error::last_os_error());
        }
        struct Fd(i32);
        impl Drop for Fd {
            fn drop(&mut self) {
                unsafe { libc::close(self.0) };
            }
        }
        let fd = Fd(fd);

        let mut pkt = [0u8; 24];
        pkt[0] = 8; // echo request
        pkt[6..8].copy_from_slice(&seq.to_be_bytes());
        pkt[8..].copy_from_slice(b"sentinelle-probe");
        let sum = checksum(&pkt);
        pkt[2..4].copy_from_slice(&sum.to_be_bytes());

        let sa = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr { s_addr: u32::from(addr).to_be() },
            sin_zero: [0; 8],
        };
        let start = Instant::now();
        let sent = libc::sendto(
            fd.0,
            pkt.as_ptr().cast(),
            pkt.len(),
            0,
            (&sa as *const libc::sockaddr_in).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as u32,
        );
        if sent < 0 {
            let e = Error::last_os_error();
            // Unreachable network etc. is a lost probe, not a broken method.
            return match e.raw_os_error() {
                Some(libc::ENETUNREACH) | Some(libc::EHOSTUNREACH) => Ok(None),
                _ => Err(e),
            };
        }
        let mut buf = [0u8; 128];
        loop {
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Ok(None);
            }
            let mut pfd = libc::pollfd { fd: fd.0, events: libc::POLLIN, revents: 0 };
            let r = libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32);
            if r <= 0 {
                return Ok(None);
            }
            let n = libc::recv(fd.0, buf.as_mut_ptr().cast(), buf.len(), 0);
            if n >= 8 && buf[0] == 0 && buf[6..8] == seq.to_be_bytes() {
                return Ok(Some(start.elapsed().as_secs_f32() * 1000.0));
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn icmp_v4(_: std::net::Ipv4Addr, _: u16, _: Duration) -> std::io::Result<Option<f32>> {
    Err(std::io::Error::other("unsupported"))
}

#[cfg(target_os = "linux")]
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for c in data.chunks(2) {
        let w = if c.len() == 2 { u16::from_be_bytes([c[0], c[1]]) } else { u16::from(c[0]) << 8 };
        sum += u32::from(w);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
