//! Persistent UI-facing state (SRT settings, recording labels/schedules, playout).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use roc_pipelines::RecordingRole;
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
fn default_playout_source() -> String {
    "srt".into()
}
fn default_caller() -> String {
    "caller".into()
}
fn default_auto_format() -> String {
    "auto".into()
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

/// Persisted decode/playout client settings (UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayoutMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_playout_source")]
    pub source: String,
    #[serde(default)]
    pub file_id: String,
    #[serde(default, rename = "loop")]
    pub loop_file: bool,
    #[serde(default = "default_caller")]
    pub mode: String,
    #[serde(default = "default_port_zero")]
    pub port: u16,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(default = "default_latency")]
    pub latency_ms: u32,
    /// `auto` = probe source on start; otherwise BMD code override (e.g. `Hp50`).
    #[serde(default = "default_auto_format")]
    pub format_code: String,
    /// Cached media duration in seconds (file source).
    #[serde(default)]
    pub duration_sec: Option<f64>,
    /// Probed video codec name (e.g. `h264`), empty if unknown.
    #[serde(default)]
    pub video_codec: String,
    /// Probed audio codec name (e.g. `aac`), empty if unknown.
    #[serde(default)]
    pub audio_codec: String,
    /// Number of discrete audio streams/tracks in the file.
    #[serde(default)]
    pub audio_tracks: u32,
    /// Total audio channels across tracks (e.g. 8 for 4×stereo).
    #[serde(default)]
    pub audio_channels: u32,
    /// Inclusive playhead start (seconds).
    #[serde(default)]
    pub mark_in_sec: f64,
    /// Exclusive playhead end (seconds). `None` = natural EOF.
    #[serde(default)]
    pub mark_out_sec: Option<f64>,
}

impl Default for PlayoutMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: default_playout_source(),
            file_id: String::new(),
            loop_file: false,
            mode: default_caller(),
            port: 0,
            target: String::new(),
            passphrase: String::new(),
            latency_ms: default_latency(),
            format_code: default_auto_format(),
            duration_sec: None,
            video_codec: String::new(),
            audio_codec: String::new(),
            audio_tracks: 0,
            audio_channels: 0,
            mark_in_sec: 0.0,
            mark_out_sec: None,
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
    /// Wall-clock when the current PROXY recording started (not persisted across restart).
    #[serde(skip)]
    pub started_at_proxy: Option<DateTime<Utc>>,
    /// Wall-clock when the current HQ recording started (not persisted across restart).
    #[serde(skip)]
    pub started_at_hq: Option<DateTime<Utc>>,
}

impl RecMeta {
    /// Start time for a REC role.
    pub fn started_at_for(&self, role: RecordingRole) -> Option<DateTime<Utc>> {
        match role {
            RecordingRole::Proxy => self.started_at_proxy,
            RecordingRole::Hq => self.started_at_hq,
        }
    }
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
            started_at_proxy: None,
            started_at_hq: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecSchedule {
    pub start_at: DateTime<Utc>,
    pub stop_at: DateTime<Utc>,
    #[serde(default)]
    pub phase: Option<String>,
    /// Arm PROXY recording for this schedule window.
    #[serde(default)]
    pub arm_proxy: bool,
    /// Arm HQ recording for this schedule window (default true for legacy schedules).
    #[serde(default = "default_true")]
    pub arm_hq: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistFile {
    #[serde(default)]
    srt: HashMap<String, SrtSettings>,
    #[serde(default)]
    recordings: HashMap<String, RecMetaPersist>,
    #[serde(default)]
    playout: HashMap<String, PlayoutMeta>,
    /// Encode capture desired-on across engine restarts (`"1": true`).
    #[serde(default)]
    encode_wanted: HashMap<String, bool>,
    #[serde(default)]
    tc: HashMap<String, TcMeta>,
    /// Per-channel workflow mode: `pair` | `tc` | `remote_commentator`.
    #[serde(default)]
    workflows: HashMap<String, String>,
    #[serde(default)]
    recordings_dir: Option<PathBuf>,
}

/// Persisted TC burn-in settings (UI + auto-start).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcMeta {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tc_source")]
    pub source: String,
    #[serde(default)]
    pub udp_port: u16,
    #[serde(default = "default_tc_fontsize")]
    pub fontsize: u32,
    #[serde(default = "default_tc_opacity")]
    pub opacity: f64,
    #[serde(default = "default_tc_position")]
    pub position: String,
}

fn default_tc_source() -> String {
    "tod".into()
}
fn default_tc_fontsize() -> u32 {
    96
}
fn default_tc_opacity() -> f64 {
    0.9
}
fn default_tc_position() -> String {
    "top_left".into()
}

impl Default for TcMeta {
    fn default() -> Self {
        Self {
            enabled: false,
            source: default_tc_source(),
            udp_port: 0,
            fontsize: default_tc_fontsize(),
            opacity: default_tc_opacity(),
            position: default_tc_position(),
        }
    }
}

impl TcMeta {
    pub fn effective_udp_port(&self, channel_id: u32) -> u16 {
        if self.udp_port > 0 {
            self.udp_port
        } else {
            9300 + channel_id as u16
        }
    }
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
    playout: HashMap<u32, PlayoutMeta>,
    /// When true (or unset), encode capture should come back after engine restart.
    encode_wanted: HashMap<u32, bool>,
    tc: HashMap<u32, TcMeta>,
    workflows: HashMap<u32, String>,
    recordings_dir: PathBuf,
}

impl UiState {
    pub fn load(data_dir: &Path, public_host: String, default_recordings: PathBuf) -> Self {
        let _ = fs::create_dir_all(data_dir);
        let path = data_dir.join("ui-state.json");
        let mut srt = HashMap::new();
        let mut recordings = HashMap::new();
        let mut playout = HashMap::new();
        let mut encode_wanted = HashMap::new();
        let mut tc = HashMap::new();
        let mut workflows = HashMap::new();
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
                                started_at_proxy: None,
                                started_at_hq: None,
                            },
                        );
                    }
                }
                for (k, v) in f.playout {
                    if let Ok(id) = k.parse::<u32>() {
                        playout.insert(id, v);
                    }
                }
                for (k, v) in f.encode_wanted {
                    if let Ok(id) = k.parse::<u32>() {
                        encode_wanted.insert(id, v);
                    }
                }
                for (k, v) in f.tc {
                    if let Ok(id) = k.parse::<u32>() {
                        tc.insert(id, v);
                    }
                }
                for (k, v) in f.workflows {
                    if let Ok(id) = k.parse::<u32>() {
                        workflows.insert(id, normalize_workflow_mode(&v));
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
                playout,
                encode_wanted,
                tc,
                workflows,
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
        for (id, p) in &guard.playout {
            f.playout.insert(id.to_string(), p.clone());
        }
        for (id, w) in &guard.encode_wanted {
            f.encode_wanted.insert(id.to_string(), *w);
        }
        for (id, t) in &guard.tc {
            f.tc.insert(id.to_string(), t.clone());
        }
        for (id, mode) in &guard.workflows {
            f.workflows.insert(id.to_string(), mode.clone());
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

    /// Whether encode capture should be restored after engine restart.
    /// `None` means unset (legacy) — treat as wanted so existing installs come back up.
    pub fn encode_wanted(&self, id: u32) -> Option<bool> {
        self.inner.lock().encode_wanted.get(&id).copied()
    }

    pub fn set_encode_wanted(&self, id: u32, wanted: bool) {
        self.inner.lock().encode_wanted.insert(id, wanted);
        self.persist();
    }

    pub fn ensure_playout(&self, id: u32, default_name: &str) {
        let mut g = self.inner.lock();
        g.playout.entry(id).or_insert_with(|| PlayoutMeta {
            name: default_name.to_string(),
            port: 9200 + id as u16,
            ..PlayoutMeta::default()
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

    pub fn playout(&self, id: u32) -> PlayoutMeta {
        self.inner
            .lock()
            .playout
            .get(&id)
            .cloned()
            .unwrap_or_else(|| PlayoutMeta {
                name: format!("Decode {id}"),
                port: 9200 + id as u16,
                ..PlayoutMeta::default()
            })
    }

    pub fn set_playout(&self, id: u32, meta: PlayoutMeta) {
        self.inner.lock().playout.insert(id, meta);
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

    pub fn set_schedule(
        &self,
        id: u32,
        start: DateTime<Utc>,
        stop: DateTime<Utc>,
        arm_proxy: bool,
        arm_hq: bool,
    ) {
        let mut g = self.inner.lock();
        let e = g.recordings.entry(id).or_default();
        e.schedule = Some(RecSchedule {
            start_at: start,
            stop_at: stop,
            phase: Some("pending".into()),
            arm_proxy,
            arm_hq,
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

    pub fn mark_recording_started_role(&self, id: u32, role: RecordingRole) {
        let mut g = self.inner.lock();
        let e = g.recordings.entry(id).or_default();
        let now = Some(Utc::now());
        match role {
            RecordingRole::Proxy => e.started_at_proxy = now,
            RecordingRole::Hq => e.started_at_hq = now,
        }
    }

    pub fn mark_recording_stopped_role(&self, id: u32, role: RecordingRole) {
        let mut g = self.inner.lock();
        if let Some(e) = g.recordings.get_mut(&id) {
            match role {
                RecordingRole::Proxy => e.started_at_proxy = None,
                RecordingRole::Hq => e.started_at_hq = None,
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
        Self::build_srt_url(
            &s.mode,
            s.port,
            &s.target,
            &s.passphrase,
            s.latency_ms,
            9100 + id as u16,
            true,
        )
    }

    /// SRT URI for decode/playout (listener or caller).
    pub fn playout_srt_url(&self, id: u32) -> anyhow::Result<String> {
        let p = self.playout(id);
        Self::build_srt_url(
            &p.mode,
            p.port,
            &p.target,
            &p.passphrase,
            p.latency_ms,
            9200 + id as u16,
            true,
        )
    }

    fn build_srt_url(
        mode: &str,
        port: u16,
        target: &str,
        passphrase: &str,
        latency_ms: u32,
        default_port: u16,
        include_passphrase: bool,
    ) -> anyhow::Result<String> {
        let latency_ms = if latency_ms == 0 { 120 } else { latency_ms };
        let latency_us = if latency_ms > 8000 {
            latency_ms
        } else {
            latency_ms * 1000
        };
        match mode {
            "caller" => {
                let target = target.trim();
                if target.is_empty() {
                    anyhow::bail!("caller mode requires a target");
                }
                if target.starts_with("srt://") {
                    // Don't append mode/latency if the URL already carries them
                    // (UI often stores a full publish URL from encode).
                    if target.contains("mode=") || target.contains("latency=") {
                        return Ok(target.to_string());
                    }
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
                let port = if port == 0 { default_port } else { port };
                let mut url = format!(
                    "srt://0.0.0.0:{port}?mode=listener&latency={latency_us}"
                );
                if include_passphrase && !passphrase.is_empty() {
                    url.push_str(&format!("&passphrase={}", urlencoding_lite(passphrase)));
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

    pub fn playout_listen_url(&self, id: u32) -> String {
        let p = self.playout(id);
        let latency = if p.latency_ms == 0 { 120 } else { p.latency_ms };
        match p.mode.as_str() {
            "caller" => {
                let t = p.target.trim();
                if t.is_empty() {
                    String::new()
                } else if t.starts_with("srt://") {
                    t.to_string()
                } else {
                    format!("srt://{t}?mode=caller&latency={latency}")
                }
            }
            _ => {
                let port = if p.port == 0 { 9200 + id as u16 } else { p.port };
                format!(
                    "srt://{}:{port}?mode=caller&latency={latency}",
                    self.public_host
                )
            }
        }
    }

    pub fn tc(&self, id: u32) -> TcMeta {
        self.inner
            .lock()
            .tc
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_tc(&self, id: u32, meta: TcMeta) {
        self.inner.lock().tc.insert(id, meta);
        self.persist();
    }

    pub fn workflow_mode(&self, id: u32) -> String {
        self.inner
            .lock()
            .workflows
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "pair".into())
    }

    pub fn set_workflow_mode(&self, id: u32, mode: &str) {
        let mode = normalize_workflow_mode(mode);
        self.inner.lock().workflows.insert(id, mode);
        self.persist();
    }
}

fn normalize_workflow_mode(mode: &str) -> String {
    match mode.trim() {
        "tc" => "tc".into(),
        "remote_commentator" => "remote_commentator".into(),
        _ => "pair".into(),
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
