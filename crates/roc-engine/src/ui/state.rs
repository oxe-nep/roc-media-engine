//! Persistent UI-facing state (SRT settings, recording labels/schedules).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrtSettings {
    #[serde(default = "default_listener")]
    pub mode: String,
    #[serde(default = "default_port_zero")]
    pub port: u16,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(default = "default_latency")]
    pub latency_ms: u32,
}

fn default_listener() -> String {
    "listener".into()
}
fn default_port_zero() -> u16 {
    0
}
fn default_latency() -> u32 {
    120
}

impl Default for SrtSettings {
    fn default() -> Self {
        Self {
            mode: default_listener(),
            port: 0,
            target: String::new(),
            passphrase: String::new(),
            latency_ms: default_latency(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub schedule: Option<RecSchedule>,
    /// Wall-clock when current recording started (not persisted across restart).
    #[serde(skip)]
    pub started_at: Option<DateTime<Utc>>,
}

fn default_category() -> String {
    "_unsorted".into()
}

impl Default for RecMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            category: default_category(),
            schedule: None,
            started_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecSchedule {
    pub start_at: DateTime<Utc>,
    pub stop_at: DateTime<Utc>,
    #[serde(default)]
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistFile {
    #[serde(default)]
    srt: HashMap<String, SrtSettings>,
    #[serde(default)]
    recordings: HashMap<String, RecMetaPersist>,
    #[serde(default)]
    recordings_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RecMetaPersist {
    #[serde(default)]
    name: String,
    #[serde(default = "default_category")]
    category: String,
    #[serde(default)]
    schedule: Option<RecSchedule>,
}

pub struct UiState {
    path: PathBuf,
    public_host: String,
    inner: Mutex<Inner>,
}

struct Inner {
    srt: HashMap<u32, SrtSettings>,
    recordings: HashMap<u32, RecMeta>,
    recordings_dir: PathBuf,
}

impl UiState {
    pub fn load(data_dir: &Path, public_host: String, default_recordings: PathBuf) -> Self {
        let _ = fs::create_dir_all(data_dir);
        let path = data_dir.join("ui-state.json");
        let mut srt = HashMap::new();
        let mut recordings = HashMap::new();
        let mut recordings_dir = default_recordings;
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(f) = serde_json::from_str::<PersistFile>(&raw) {
                for (k, v) in f.srt {
                    if let Ok(id) = k.parse::<u32>() {
                        srt.insert(id, v);
                    }
                }
                for (k, v) in f.recordings {
                    if let Ok(id) = k.parse::<u32>() {
                        recordings.insert(
                            id,
                            RecMeta {
                                name: v.name,
                                category: if v.category.is_empty() {
                                    default_category()
                                } else {
                                    v.category
                                },
                                schedule: v.schedule,
                                started_at: None,
                            },
                        );
                    }
                }
                if let Some(p) = f.recordings_dir {
                    if !p.as_os_str().is_empty() {
                        recordings_dir = p;
                    }
                }
            }
        }
        Self {
            path,
            public_host,
            inner: Mutex::new(Inner {
                srt,
                recordings,
                recordings_dir,
            }),
        }
    }

    pub fn persist(&self) {
        let guard = self.inner.lock();
        let mut f = PersistFile::default();
        for (id, s) in &guard.srt {
            f.srt.insert(id.to_string(), s.clone());
        }
        for (id, r) in &guard.recordings {
            f.recordings.insert(
                id.to_string(),
                RecMetaPersist {
                    name: r.name.clone(),
                    category: r.category.clone(),
                    schedule: r.schedule.clone(),
                },
            );
        }
        f.recordings_dir = Some(guard.recordings_dir.clone());
        drop(guard);
        if let Ok(raw) = serde_json::to_string_pretty(&f) {
            let _ = fs::write(&self.path, raw);
        }
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.inner.lock().recordings_dir.clone()
    }

    pub fn set_recordings_dir(&self, path: PathBuf) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(&path)?;
        self.inner.lock().recordings_dir = path.clone();
        self.persist();
        Ok(path)
    }

    pub fn ensure_channel(&self, id: u32, default_name: &str) {
        let mut g = self.inner.lock();
        g.srt.entry(id).or_insert_with(|| SrtSettings {
            port: 9100 + id as u16,
            ..SrtSettings::default()
        });
        g.recordings.entry(id).or_insert_with(|| RecMeta {
            name: default_name.to_string(),
            ..RecMeta::default()
        });
    }

    pub fn srt(&self, id: u32) -> SrtSettings {
        self.inner
            .lock()
            .srt
            .get(&id)
            .cloned()
            .unwrap_or_else(|| SrtSettings {
                port: 9100 + id as u16,
                ..SrtSettings::default()
            })
    }

    pub fn set_srt(&self, id: u32, settings: SrtSettings) {
        self.inner.lock().srt.insert(id, settings);
        self.persist();
    }

    pub fn rec_meta(&self, id: u32) -> RecMeta {
        self.inner
            .lock()
            .recordings
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_rec_name(&self, id: u32, name: String) {
        let mut g = self.inner.lock();
        let e = g.recordings.entry(id).or_default();
        e.name = name;
        drop(g);
        self.persist();
    }

    pub fn set_rec_category(&self, id: u32, category: String) {
        let mut g = self.inner.lock();
        let e = g.recordings.entry(id).or_default();
        e.category = if category.trim().is_empty() {
            default_category()
        } else {
            category
        };
        drop(g);
        self.persist();
    }

    pub fn set_schedule(&self, id: u32, start: DateTime<Utc>, stop: DateTime<Utc>) {
        let mut g = self.inner.lock();
        let e = g.recordings.entry(id).or_default();
        e.schedule = Some(RecSchedule {
            start_at: start,
            stop_at: stop,
            phase: Some("pending".into()),
        });
        drop(g);
        self.persist();
    }

    pub fn clear_schedule(&self, id: u32) {
        let mut g = self.inner.lock();
        if let Some(e) = g.recordings.get_mut(&id) {
            e.schedule = None;
        }
        drop(g);
        self.persist();
    }

    pub fn mark_recording_started(&self, id: u32) {
        let mut g = self.inner.lock();
        let e = g.recordings.entry(id).or_default();
        e.started_at = Some(Utc::now());
    }

    pub fn mark_recording_stopped(&self, id: u32) {
        let mut g = self.inner.lock();
        if let Some(e) = g.recordings.get_mut(&id) {
            e.started_at = None;
            if let Some(s) = e.schedule.as_mut() {
                s.phase = None;
            }
        }
    }

    pub fn all_schedules(&self) -> Vec<(u32, RecSchedule)> {
        self.inner
            .lock()
            .recordings
            .iter()
            .filter_map(|(id, m)| m.schedule.clone().map(|s| (*id, s)))
            .collect()
    }

    /// Build engine publish URI (latency in microseconds for GST/FFmpeg compat).
    pub fn srt_output_url(&self, id: u32) -> anyhow::Result<String> {
        let s = self.srt(id);
        let latency_ms = if s.latency_ms == 0 { 120 } else { s.latency_ms };
        let latency_us = if latency_ms > 8000 {
            latency_ms
        } else {
            latency_ms * 1000
        };
        match s.mode.as_str() {
            "caller" => {
                let target = s.target.trim();
                if target.is_empty() {
                    anyhow::bail!("caller mode requires a target");
                }
                if target.starts_with("srt://") {
                    let sep = if target.contains('?') { '&' } else { '?' };
                    Ok(format!(
                        "{target}{sep}mode=caller&latency={latency_us}"
                    ))
                } else {
                    Ok(format!(
                        "srt://{target}?mode=caller&latency={latency_us}"
                    ))
                }
            }
            _ => {
                let port = if s.port == 0 { 9100 + id as u16 } else { s.port };
                let mut url = format!(
                    "srt://0.0.0.0:{port}?mode=listener&latency={latency_us}"
                );
                if !s.passphrase.is_empty() {
                    url.push_str(&format!("&passphrase={}", urlencoding_lite(&s.passphrase)));
                }
                Ok(url)
            }
        }
    }

    pub fn srt_publish_url(&self, id: u32) -> String {
        let s = self.srt(id);
        let latency = if s.latency_ms == 0 { 120 } else { s.latency_ms };
        match s.mode.as_str() {
            "caller" => {
                let t = s.target.trim();
                if t.is_empty() {
                    String::new()
                } else if t.starts_with("srt://") {
                    t.to_string()
                } else {
                    format!("srt://{t}?mode=caller&latency={latency}")
                }
            }
            _ => {
                let port = if s.port == 0 { 9100 + id as u16 } else { s.port };
                format!(
                    "srt://{}:{port}?mode=caller&latency={latency}",
                    self.public_host
                )
            }
        }
    }
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
