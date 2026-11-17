//! Host / GPU metrics for `/api/system` (ported from roc-recording Go collector).
//!
//! One background sampler ([`spawn_sampler`]) reads everything every
//! [`SAMPLE_EVERY`] and keeps [`HISTORY_LEN`] points of history. Requests only
//! read the latest sample, so CPU / network rates are deltas over a fixed
//! interval instead of "time since some other browser last asked".

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::orchestrator::Orchestrator;

pub const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// 15 minutes at 2 s.
pub const HISTORY_LEN: usize = 450;

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub mem_percent: f64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_percent: f64,
    pub disk_path: String,
    pub gpu_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvenc_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvdec_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_mem_used_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_mem_total_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_power_w: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_power_limit_w: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_encoder_sessions: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_encoder_fps: Option<f64>,
    pub hostname: String,
    pub cpu_cores: usize,
    pub load_avg: [f64; 3],
    pub host_uptime_secs: u64,
    pub engine_uptime_secs: u64,
    /// Physical NICs only (bits/s, summed). None until the second sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_rx_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_tx_bps: Option<f64>,
    pub net_ifaces: Vec<String>,
    /// Sum of live encode bitrates across channels (kbps).
    pub encode_kbps_total: f64,
    /// Sum of live SRT output bitrates across channels (kbps).
    pub srt_kbps_total: f64,
    pub sampled_at_ms: u64,
}

/// One history point (compact keys: 450 of these go over the wire).
#[derive(Debug, Clone, Serialize)]
pub struct HistPoint {
    pub t: u64,
    pub cpu: f64,
    pub mem: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvenc: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvdec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_mbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_mbps: Option<f64>,
    pub enc_mbps: f64,
    pub srt_mbps: f64,
}

impl HistPoint {
    fn from_snapshot(s: &Snapshot) -> Self {
        let r1 = |v: f64| (v * 10.0).round() / 10.0;
        Self {
            t: s.sampled_at_ms,
            cpu: r1(s.cpu_percent),
            mem: r1(s.mem_percent),
            gpu: s.gpu_percent,
            nvenc: s.nvenc_percent,
            nvdec: s.nvdec_percent,
            vram: match (s.gpu_mem_used_mb, s.gpu_mem_total_mb) {
                (Some(u), Some(t)) if t > 0.0 => Some(r1(u / t * 100.0)),
                _ => None,
            },
            temp: s.gpu_temp_c,
            rx_mbps: s.net_rx_bps.map(|v| r1(v / 1e6)),
            tx_mbps: s.net_tx_bps.map(|v| r1(v / 1e6)),
            enc_mbps: r1(s.encode_kbps_total / 1000.0),
            srt_mbps: r1(s.srt_kbps_total / 1000.0),
        }
    }
}

pub struct Collector {
    inner: Mutex<Inner>,
    latest: Mutex<Option<Snapshot>>,
    history: Mutex<VecDeque<HistPoint>>,
    started: Instant,
}

struct Inner {
    prev_cpu: Option<(u64, u64)>,
    prev_net: Option<(Instant, u64, u64)>,
    disk_path: PathBuf,
}

impl Collector {
    pub fn new(disk_path: PathBuf) -> Self {
        let path = if disk_path.as_os_str().is_empty() {
            PathBuf::from("/")
        } else {
            disk_path
        };
        Self {
            inner: Mutex::new(Inner {
                prev_cpu: read_cpu_times().ok(),
                prev_net: None,
                disk_path: path,
            }),
            latest: Mutex::new(None),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_LEN)),
            started: Instant::now(),
        }
    }

    pub fn set_disk_path(&self, path: PathBuf) {
        let mut g = self.inner.lock().unwrap();
        g.disk_path = if path.as_os_str().is_empty() {
            PathBuf::from("/")
        } else {
            path
        };
    }

    pub fn disk_path(&self) -> PathBuf {
        self.inner.lock().unwrap().disk_path.clone()
    }

    /// Latest sample; takes one synchronously if the sampler has not run yet.
    pub fn snapshot(&self) -> Snapshot {
        if let Some(s) = self.latest.lock().unwrap().clone() {
            return s;
        }
        self.sample(0.0, 0.0)
    }

    pub fn history(&self) -> Vec<HistPoint> {
        self.history.lock().unwrap().iter().cloned().collect()
    }

    /// Read every metric now, store it as latest and append to history.
    pub fn sample(&self, encode_kbps_total: f64, srt_kbps_total: f64) -> Snapshot {
        let disk_path = self.disk_path();
        let gpu = gpu_stats();
        let mut s = Snapshot {
            cpu_percent: self.cpu_percent().unwrap_or(0.0),
            mem_used_bytes: 0,
            mem_total_bytes: 0,
            mem_percent: 0.0,
            disk_used_bytes: 0,
            disk_total_bytes: 0,
            disk_percent: 0.0,
            disk_path: disk_path.display().to_string(),
            gpu_available: gpu.is_some(),
            nvenc_percent: gpu.as_ref().map(|g| g.nvenc),
            nvdec_percent: gpu.as_ref().map(|g| g.nvdec),
            gpu_percent: gpu.as_ref().map(|g| g.util),
            gpu_mem_used_mb: gpu.as_ref().map(|g| g.mem_used),
            gpu_mem_total_mb: gpu.as_ref().map(|g| g.mem_total),
            gpu_name: gpu.as_ref().and_then(|g| g.name.clone()),
            gpu_driver: gpu.as_ref().and_then(|g| g.driver.clone()),
            gpu_temp_c: gpu.as_ref().and_then(|g| g.temp),
            gpu_power_w: gpu.as_ref().and_then(|g| g.power),
            gpu_power_limit_w: gpu.as_ref().and_then(|g| g.power_limit),
            gpu_encoder_sessions: gpu.as_ref().and_then(|g| g.enc_sessions),
            gpu_encoder_fps: gpu.as_ref().and_then(|g| g.enc_fps),
            hostname: read_trimmed("/proc/sys/kernel/hostname").unwrap_or_default(),
            cpu_cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            load_avg: load_avg().unwrap_or([0.0; 3]),
            host_uptime_secs: host_uptime().unwrap_or(0),
            engine_uptime_secs: self.started.elapsed().as_secs(),
            net_rx_bps: None,
            net_tx_bps: None,
            net_ifaces: Vec::new(),
            encode_kbps_total,
            srt_kbps_total,
            sampled_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        };
        if let Ok((used, total)) = mem_usage() {
            if total > 0 {
                s.mem_used_bytes = used;
                s.mem_total_bytes = total;
                s.mem_percent = used as f64 / total as f64 * 100.0;
            }
        }
        if let Ok((used, total)) = disk_usage(&disk_path) {
            if total > 0 {
                s.disk_used_bytes = used;
                s.disk_total_bytes = total;
                s.disk_percent = used as f64 / total as f64 * 100.0;
            }
        }
        if let Ok((ifaces, rx, tx)) = net_bytes() {
            s.net_ifaces = ifaces;
            let now = Instant::now();
            let mut g = self.inner.lock().unwrap();
            if let Some((at, prx, ptx)) = g.prev_net {
                let dt = now.duration_since(at).as_secs_f64();
                if dt > 0.2 {
                    s.net_rx_bps = Some(rx.saturating_sub(prx) as f64 * 8.0 / dt);
                    s.net_tx_bps = Some(tx.saturating_sub(ptx) as f64 * 8.0 / dt);
                }
            }
            g.prev_net = Some((now, rx, tx));
        }
        *self.latest.lock().unwrap() = Some(s.clone());
        let mut h = self.history.lock().unwrap();
        if h.len() >= HISTORY_LEN {
            h.pop_front();
        }
        h.push_back(HistPoint::from_snapshot(&s));
        s
    }

    fn cpu_percent(&self) -> std::io::Result<f64> {
        let (idle, total) = read_cpu_times()?;
        let mut g = self.inner.lock().unwrap();
        let prev = g.prev_cpu.replace((idle, total));
        let Some((p_idle, p_total)) = prev else {
            return Ok(0.0);
        };
        let d_idle = idle.saturating_sub(p_idle);
        let d_total = total.saturating_sub(p_total);
        if d_total == 0 {
            return Ok(0.0);
        }
        Ok(((1.0 - d_idle as f64 / d_total as f64) * 100.0).clamp(0.0, 100.0))
    }
}

/// Sample every [`SAMPLE_EVERY`], UI open or not, so history is there when
/// someone opens the status view.
pub fn spawn_sampler(sys: Arc<Collector>, orch: Arc<Orchestrator>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SAMPLE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let (s, o) = (sys.clone(), orch.clone());
            let res = tokio::task::spawn_blocking(move || {
                // Meter snapshot: bitrates without format adapt / relaunch.
                let chans = o.list_channel_meters();
                let enc: f64 = chans.iter().filter_map(|c| c.video_bitrate_kbps).sum();
                let srt: f64 = chans.iter().filter_map(|c| c.srt_bitrate_kbps).sum();
                s.sample(enc, srt);
            })
            .await;
            if let Err(e) = res {
                tracing::error!(error = %e, "system metrics sample panicked");
            }
        }
    });
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn load_avg() -> Option<[f64; 3]> {
    let s = read_trimmed("/proc/loadavg")?;
    let mut it = s.split_whitespace().map(|v| v.parse::<f64>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

fn host_uptime() -> Option<u64> {
    let s = read_trimmed("/proc/uptime")?;
    s.split_whitespace().next()?.parse::<f64>().ok().map(|v| v as u64)
}

/// Summed rx/tx bytes over physical NICs (those with a `device` link in sysfs —
/// skips lo, docker, veth, cni, flannel and other virtual interfaces).
fn net_bytes() -> std::io::Result<(Vec<String>, u64, u64)> {
    let f = File::open("/proc/net/dev")?;
    let mut names = Vec::new();
    let (mut rx, mut tx) = (0u64, 0u64);
    for line in BufReader::new(f).lines().map_while(Result::ok).skip(2) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if !Path::new("/sys/class/net").join(name).join("device").exists() {
            continue;
        }
        let vals: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        if vals.len() < 9 {
            continue;
        }
        rx += vals[0];
        tx += vals[8];
        names.push(name.to_string());
    }
    Ok((names, rx, tx))
}

fn read_cpu_times() -> std::io::Result<(u64, u64)> {
    let f = File::open("/proc/stat")?;
    let mut line = String::new();
    BufReader::new(f).read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    if parts.next() != Some("cpu") {
        return Err(std::io::Error::other("unexpected /proc/stat"));
    }
    let vals: Vec<u64> = parts.filter_map(|p| p.parse().ok()).collect();
    if vals.len() < 4 {
        return Err(std::io::Error::other("short /proc/stat"));
    }
    let mut idle = vals[3];
    if vals.len() > 4 {
        idle += vals[4];
    }
    let total: u64 = vals.iter().sum();
    Ok((idle, total))
}

fn mem_usage() -> std::io::Result<(u64, u64)> {
    let f = File::open("/proc/meminfo")?;
    let mut mem_total = 0u64;
    let mut mem_avail = 0u64;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            mem_total = parse_mem_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            mem_avail = parse_mem_kb(rest);
        }
    }
    if mem_total == 0 {
        return Err(std::io::Error::other("MemTotal missing"));
    }
    let used = (mem_total.saturating_sub(mem_avail)) * 1024;
    Ok((used, mem_total * 1024))
}

fn parse_mem_kb(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn disk_usage(path: &Path) -> std::io::Result<(u64, u64)> {
    // Prefer the recordings path; fall back to /
    let target = if path.exists() {
        path
    } else {
        Path::new("/")
    };
    let out = Command::new("df")
        .args(["-B1", "--output=used,size", target.to_str().unwrap_or("/")])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other("df failed"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        let mut parts = line.split_whitespace();
        let used: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let total: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        if total > 0 {
            return Ok((used, total));
        }
    }
    Err(std::io::Error::other("df parse failed"))
}

struct GpuStats {
    nvenc: f64,
    nvdec: f64,
    util: f64,
    mem_used: f64,
    mem_total: f64,
    temp: Option<f64>,
    power: Option<f64>,
    power_limit: Option<f64>,
    enc_sessions: Option<u32>,
    enc_fps: Option<f64>,
    name: Option<String>,
    driver: Option<String>,
}

fn gpu_stats() -> Option<GpuStats> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.encoder,utilization.decoder,utilization.gpu,memory.used,memory.total,\
temperature.gpu,power.draw,power.limit,encoder.stats.sessionCount,encoder.stats.averageFps,name,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    parse_gpu_line(line.lines().next()?.trim())
}

fn parse_gpu_line(line: &str) -> Option<GpuStats> {
    let p: Vec<&str> = line.split(',').map(str::trim).collect();
    if p.len() < 5 {
        return None;
    }
    // Optional fields report "[N/A]" / "[Not Supported]" on some boards.
    let opt_f = |i: usize| p.get(i).and_then(|v| v.parse::<f64>().ok());
    let opt_s = |i: usize| {
        p.get(i)
            .filter(|v| !v.is_empty() && !v.starts_with('['))
            .map(|v| v.to_string())
    };
    Some(GpuStats {
        nvenc: p[0].parse().ok()?,
        nvdec: p[1].parse().ok()?,
        util: p[2].parse().ok()?,
        mem_used: p[3].parse().ok()?,
        mem_total: p[4].parse().ok()?,
        temp: opt_f(5),
        power: opt_f(6),
        power_limit: opt_f(7),
        enc_sessions: p.get(8).and_then(|v| v.parse::<u32>().ok()),
        enc_fps: opt_f(9),
        name: opt_s(10),
        driver: opt_s(11),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_gpu_line() {
        let g = parse_gpu_line("12, 3, 25, 4096, 24576, 61, 85.31, 300.00, 7, 50, NVIDIA RTX A5000, 580.95").unwrap();
        assert_eq!(g.nvenc, 12.0);
        assert_eq!(g.temp, Some(61.0));
        assert_eq!(g.power, Some(85.31));
        assert_eq!(g.enc_sessions, Some(7));
        assert_eq!(g.name.as_deref(), Some("NVIDIA RTX A5000"));
        assert_eq!(g.driver.as_deref(), Some("580.95"));
    }

    #[test]
    fn tolerates_unsupported_fields() {
        let g = parse_gpu_line("0, 0, 1, 100, 8000, 40, [N/A], [N/A], 0, 0, Tesla T4, 550.1").unwrap();
        assert_eq!(g.power, None);
        assert_eq!(g.enc_sessions, Some(0));
        assert!(parse_gpu_line("a, b").is_none());
    }
}
