//! Host / GPU metrics for `/api/system` (ported from roc-recording Go collector).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

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
}

pub struct Collector {
    inner: Mutex<Inner>,
}

struct Inner {
    prev_idle: u64,
    prev_total: u64,
    have_prev: bool,
    disk_path: PathBuf,
    gpu_at: Option<Instant>,
    gpu_ok: bool,
    gpu_nvenc: f64,
    gpu_nvdec: f64,
    gpu_util: f64,
    gpu_mem_used: f64,
    gpu_mem_total: f64,
}

impl Collector {
    pub fn new(disk_path: PathBuf) -> Self {
        let path = if disk_path.as_os_str().is_empty() {
            PathBuf::from("/")
        } else {
            disk_path
        };
        let c = Self {
            inner: Mutex::new(Inner {
                prev_idle: 0,
                prev_total: 0,
                have_prev: false,
                disk_path: path,
                gpu_at: None,
                gpu_ok: false,
                gpu_nvenc: 0.0,
                gpu_nvdec: 0.0,
                gpu_util: 0.0,
                gpu_mem_used: 0.0,
                gpu_mem_total: 0.0,
            }),
        };
        let _ = c.cpu_percent();
        c
    }

    pub fn set_disk_path(&self, path: PathBuf) {
        let mut g = self.inner.lock().unwrap();
        g.disk_path = if path.as_os_str().is_empty() {
            PathBuf::from("/")
        } else {
            path
        };
    }

    pub fn snapshot(&self) -> Snapshot {
        let disk_path = self.inner.lock().unwrap().disk_path.clone();
        let mut s = Snapshot {
            cpu_percent: 0.0,
            mem_used_bytes: 0,
            mem_total_bytes: 0,
            mem_percent: 0.0,
            disk_used_bytes: 0,
            disk_total_bytes: 0,
            disk_percent: 0.0,
            disk_path: disk_path.display().to_string(),
            gpu_available: false,
            nvenc_percent: None,
            nvdec_percent: None,
            gpu_percent: None,
            gpu_mem_used_mb: None,
            gpu_mem_total_mb: None,
        };
        if let Ok(pct) = self.cpu_percent() {
            s.cpu_percent = pct;
        }
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
        if let Some((nvenc, nvdec, gpu, mem_used, mem_total)) = self.cached_gpu() {
            s.gpu_available = true;
            s.nvenc_percent = Some(nvenc);
            s.nvdec_percent = Some(nvdec);
            s.gpu_percent = Some(gpu);
            s.gpu_mem_used_mb = Some(mem_used);
            s.gpu_mem_total_mb = Some(mem_total);
        }
        s
    }

    fn cpu_percent(&self) -> std::io::Result<f64> {
        let (idle, total) = read_cpu_times()?;
        let mut g = self.inner.lock().unwrap();
        if !g.have_prev {
            g.prev_idle = idle;
            g.prev_total = total;
            g.have_prev = true;
            drop(g);
            std::thread::sleep(Duration::from_millis(120));
            let (idle2, total2) = read_cpu_times()?;
            let mut g = self.inner.lock().unwrap();
            g.prev_idle = idle2;
            g.prev_total = total2;
            let d_idle = idle2.saturating_sub(idle);
            let d_total = total2.saturating_sub(total);
            if d_total == 0 {
                return Ok(0.0);
            }
            return Ok((1.0 - d_idle as f64 / d_total as f64) * 100.0);
        }
        let d_idle = idle.saturating_sub(g.prev_idle);
        let d_total = total.saturating_sub(g.prev_total);
        g.prev_idle = idle;
        g.prev_total = total;
        if d_total == 0 {
            return Ok(0.0);
        }
        Ok(((1.0 - d_idle as f64 / d_total as f64) * 100.0).clamp(0.0, 100.0))
    }

    fn cached_gpu(&self) -> Option<(f64, f64, f64, f64, f64)> {
        {
            let g = self.inner.lock().unwrap();
            if let Some(at) = g.gpu_at {
                if at.elapsed() < Duration::from_secs(2) {
                    return if g.gpu_ok {
                        Some((
                            g.gpu_nvenc,
                            g.gpu_nvdec,
                            g.gpu_util,
                            g.gpu_mem_used,
                            g.gpu_mem_total,
                        ))
                    } else {
                        None
                    };
                }
            }
        }
        let stats = gpu_stats();
        let mut g = self.inner.lock().unwrap();
        g.gpu_at = Some(Instant::now());
        if let Some((nvenc, nvdec, util, mem_used, mem_total)) = stats {
            g.gpu_ok = true;
            g.gpu_nvenc = nvenc;
            g.gpu_nvdec = nvdec;
            g.gpu_util = util;
            g.gpu_mem_used = mem_used;
            g.gpu_mem_total = mem_total;
            Some((nvenc, nvdec, util, mem_used, mem_total))
        } else {
            g.gpu_ok = false;
            None
        }
    }
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
    for line in BufReader::new(f).lines().flatten() {
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

fn gpu_stats() -> Option<(f64, f64, f64, f64, f64)> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.encoder,utilization.decoder,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.lines().next()?.trim();
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() < 5 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
        parts[3].parse().ok()?,
        parts[4].parse().ok()?,
    ))
}
