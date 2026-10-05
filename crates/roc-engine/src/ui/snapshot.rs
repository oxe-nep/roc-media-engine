use chrono::Utc;
use roc_pipelines::{ChannelStatus, RecordingRole};
use serde::Serialize;
use serde_json::{json, Value};

use crate::orchestrator::Orchestrator;
use crate::ui::state::{RecMeta, UiState};

#[derive(Debug, Serialize)]
pub struct MeterLevels {
    pub l: f64,
    pub r: f64,
    pub channels: Vec<f64>,
}

fn status_str(s: ChannelStatus) -> &'static str {
    match s {
        ChannelStatus::Stopped => "stopped",
        ChannelStatus::Waiting => "waiting",
        ChannelStatus::Running => "running",
        ChannelStatus::Paused => "paused",
        ChannelStatus::Error => "error",
        ChannelStatus::Restarting => "restarting",
    }
}

fn silence_peaks() -> Vec<f64> {
    vec![-90.0; 8]
}

fn from_peaks(peaks: Option<&[f64]>) -> MeterLevels {
    let mut ch = silence_peaks();
    if let Some(p) = peaks {
        for (i, v) in p.iter().take(8).enumerate() {
            ch[i] = *v;
        }
    }
    MeterLevels {
        l: ch[0],
        r: ch[1],
        channels: ch,
    }
}

pub fn stream_json(orch: &Orchestrator, _ui: &UiState, id: u32) -> Option<Value> {
    let ch = orch.channel(id).ok()?;
    let status = match ch.status {
        ChannelStatus::Stopped => "stopped",
        ChannelStatus::Waiting => "waiting",
        ChannelStatus::Running => "running",
        ChannelStatus::Paused => "paused",
        ChannelStatus::Error => "error",
        ChannelStatus::Restarting => "waiting",
    };
    Some(json!({
        "id": ch.id,
        "name": ch.name,
        "status": status,
        "error": ch.last_error.unwrap_or_default(),
        "format": ch.input_format.unwrap_or_default(),
        "encode_preset": ch.encode_preset,
        "record_preset": ch.record_preset,
        "hls_url": format!("/hls/{}/preview.m3u8", ch.id),
        "preview_epoch": ch.preview_epoch,
        "backend": "media_engine",
    }))
}

/// One REC role (`proxy` / `hq`) as exposed under `recording_json`.
struct RoleRec {
    recording: bool,
    file_path: Option<String>,
    elapsed_sec: f64,
    started_at: Option<String>,
}

fn role_rec(ch: Option<&roc_pipelines::ChannelSnapshot>, meta: &RecMeta, role: RecordingRole) -> RoleRec {
    let (recording, file_path) = match (ch, role) {
        (Some(c), RecordingRole::Proxy) => (c.proxy_recording, c.proxy_recording_path.clone()),
        (Some(c), RecordingRole::Hq) => (c.hq_recording, c.hq_recording_path.clone()),
        (None, _) => (false, None),
    };
    let mut elapsed_sec = 0.0;
    let mut started_at = None;
    if recording {
        if let Some(t) = meta.started_at_for(role) {
            elapsed_sec = (Utc::now() - t).num_milliseconds() as f64 / 1000.0;
            started_at = Some(t.to_rfc3339());
        }
    }
    RoleRec {
        recording,
        file_path: if recording { file_path } else { None },
        elapsed_sec,
        started_at,
    }
}

fn role_json(r: &RoleRec) -> Value {
    json!({
        "status": if r.recording { "recording" } else { "idle" },
        "file_path": r.file_path,
        "elapsed_sec": r.elapsed_sec,
        "started_at": r.started_at,
        "encoding": r.recording,
    })
}

/// Recording state for one channel.
///
/// Top-level fields keep the legacy single-REC shape (`status`, `file_path`,
/// `elapsed_sec`, `encoding`) and report HQ first, then proxy. Nested `proxy` /
/// `hq` objects carry the per-role state for dual recording.
pub fn recording_json(orch: &Orchestrator, ui: &UiState, id: u32) -> Value {
    let ch = orch.channel(id).ok();
    let meta = ui.rec_meta(id);
    let proxy = role_rec(ch.as_ref(), &meta, RecordingRole::Proxy);
    let hq = role_rec(ch.as_ref(), &meta, RecordingRole::Hq);
    let recording = proxy.recording || hq.recording;

    // Legacy top-level values: HQ wins when both run, otherwise whichever is active.
    let primary = if hq.recording { &hq } else { &proxy };
    let elapsed = if recording { primary.elapsed_sec } else { 0.0 };
    let started_at = if recording { primary.started_at.clone() } else { None };
    let file_path = if recording {
        primary.file_path.clone()
    } else {
        None
    };

    let schedule = meta.schedule.as_ref().map(|s| {
        let now = Utc::now();
        let phase = if now < s.start_at {
            "pending"
        } else if now < s.stop_at {
            "active"
        } else {
            "pending"
        };
        json!({
            "start_at": s.start_at.to_rfc3339(),
            "stop_at": s.stop_at.to_rfc3339(),
            "phase": phase,
            "arm_proxy": s.arm_proxy,
            "arm_hq": s.arm_hq,
        })
    });
    let name = if meta.name.is_empty() {
        ch.as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("Channel {id}"))
    } else {
        meta.name.clone()
    };
    let bitrate = ch.as_ref().and_then(|c| c.video_bitrate_kbps).unwrap_or(0.0);
    json!({
        "id": id,
        "status": if recording { "recording" } else { "idle" },
        "name": name,
        "category": meta.category,
        "started_at": started_at,
        "file_path": file_path,
        "elapsed_sec": elapsed,
        "bitrate_kbps": bitrate,
        "encoding": recording,
        "schedule": schedule,
        "proxy": role_json(&proxy),
        "hq": role_json(&hq),
    })
}

pub fn srt_json(orch: &Orchestrator, ui: &UiState, id: u32) -> Value {
    let ch = orch.channel(id).ok();
    let s = ui.srt(id);
    let streaming = ch.as_ref().map(|c| c.srt).unwrap_or(false);
    let br = ch.as_ref().and_then(|c| c.srt_bitrate_kbps).unwrap_or(0.0);
    let sending = streaming && br > 0.0;
    let port = if s.port == 0 { 9100 + id as u16 } else { s.port };
    json!({
        "id": id,
        "status": if streaming { "streaming" } else { "idle" },
        "mode": s.mode,
        "port": port,
        "target": s.target,
        "has_passphrase": !s.passphrase.is_empty(),
        "latency_ms": if s.latency_ms == 0 { 120 } else { s.latency_ms },
        "publish_url": ui.srt_publish_url(id),
        "error": ch.and_then(|c| c.last_error).unwrap_or_default(),
        "bitrate_kbps": br,
        "sending": sending,
    })
}

pub fn playout_json(orch: &Orchestrator, ui: &UiState) -> Vec<Value> {
    let mut out = Vec::new();
    for p in orch.list_playout() {
        let num_id = p
            .id
            .strip_prefix("decode-")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if num_id == 0 {
            continue;
        }
        ui.ensure_playout(num_id, &p.name);
        let mut meta = ui.playout(num_id);
        if meta.source == "file" && meta.duration_sec.is_none() {
            if let Some(d) = p.duration_sec {
                meta.duration_sec = Some(d);
                ui.set_playout(num_id, meta.clone());
            }
        }
        let live = matches!(
            p.status,
            ChannelStatus::Running | ChannelStatus::Waiting | ChannelStatus::Paused
        );
        let status = if live {
            status_str(p.status)
        } else {
            "stopped"
        };
        let source_kind = if meta.source == "file" {
            "file"
        } else {
            "srt"
        };
        let format_code = if live {
            p.format_code
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if meta.format_code.is_empty() {
                        "auto".into()
                    } else {
                        meta.format_code.clone()
                    }
                })
        } else if meta.format_code.is_empty() {
            "auto".into()
        } else {
            meta.format_code.clone()
        };
        let file_name = if meta.file_id.starts_with("lib:") {
            meta.file_id
                .strip_prefix("lib:")
                .map(|s| {
                    percent_decode(s)
                        .split('/')
                        .next_back()
                        .unwrap_or(s)
                        .to_string()
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let port = if meta.port == 0 {
            9200 + num_id as u16
        } else {
            meta.port
        };
        let duration_sec = p.duration_sec.or(meta.duration_sec);
        let elapsed_sec = p.position_sec.unwrap_or(0.0);
        let remain_sec = (duration_sec.unwrap_or(0.0) - elapsed_sec).max(0.0);
        out.push(json!({
            "id": num_id,
            "name": if meta.name.is_empty() { p.name.clone() } else { meta.name.clone() },
            "status": status,
            "device": p.device,
            "device_label": p.device,
            "format_code": format_code,
            "decklink_out": true,
            "fixed": true,
            "source": source_kind,
            "file_id": meta.file_id,
            "file_name": file_name,
            "loop": meta.loop_file,
            "mark_in_sec": meta.mark_in_sec,
            "mark_out_sec": meta.mark_out_sec,
            "duration_sec": duration_sec,
            "elapsed_sec": elapsed_sec,
            "remain_sec": remain_sec,
            "mode": if meta.mode == "listener" { "listener" } else { "caller" },
            "port": port,
            "target": meta.target,
            "has_passphrase": !meta.passphrase.is_empty(),
            "latency_ms": if meta.latency_ms == 0 { 120 } else { meta.latency_ms },
            "sending": matches!(p.status, ChannelStatus::Running),
            "reconnects": 0,
            "error": p.last_error.unwrap_or_default(),
            "listen_url": ui.playout_listen_url(num_id),
        }));
    }
    out.sort_by_key(|v| v.get("id").and_then(|x| x.as_u64()).unwrap_or(0));
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn meters_maps(orch: &Orchestrator) -> (serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
    let mut enc = serde_json::Map::new();
    let mut play = serde_json::Map::new();
    for ch in orch.list_channels() {
        let m = from_peaks(ch.audio_peaks.as_deref());
        let v = serde_json::to_value(m).unwrap_or(json!({}));
        enc.insert(ch.id.to_string(), v);
    }
    for p in orch.list_playout() {
        let num_id = p
            .id
            .strip_prefix("decode-")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if num_id == 0 {
            continue;
        }
        let m = from_peaks(p.audio_peaks.as_deref());
        play.insert(
            num_id.to_string(),
            serde_json::to_value(m).unwrap_or(json!({"l": -90.0, "r": -90.0, "channels": silence_peaks()})),
        );
    }
    (enc, play)
}

pub fn dashboard_snapshot(orch: &Orchestrator, ui: &UiState) -> Value {
    let channels = orch.list_channels();
    let mut streams = Vec::new();
    let mut recordings = Vec::new();
    let mut srt = Vec::new();
    let mut workflows = serde_json::Map::new();
    for ch in &channels {
        ui.ensure_channel(ch.id, &ch.name);
        if let Some(s) = stream_json(orch, ui, ch.id) {
            streams.push(s);
        }
        recordings.push(recording_json(orch, ui, ch.id));
        srt.push(srt_json(orch, ui, ch.id));
        workflows.insert(
            ch.id.to_string(),
            json!({
                "pair": false,
                "tc": false,
                "commentator": false,
            }),
        );
    }
    streams.sort_by_key(|v| v.get("id").and_then(|x| x.as_u64()).unwrap_or(0));
    recordings.sort_by_key(|v| v.get("id").and_then(|x| x.as_u64()).unwrap_or(0));
    srt.sort_by_key(|v| v.get("id").and_then(|x| x.as_u64()).unwrap_or(0));
    let (meters_encode, meters_playout) = meters_maps(orch);
    json!({
        "type": "snapshot",
        "streams": streams,
        "playout": playout_json(orch, ui),
        "tc": [],
        "commentator": [],
        "recordings": recordings,
        "srt": srt,
        "workflows": workflows,
        "meters_encode": meters_encode,
        "meters_playout": meters_playout,
    })
}

pub fn meters_frame(orch: &Orchestrator) -> Value {
    let (meters_encode, meters_playout) = meters_maps(orch);
    json!({
        "type": "meters",
        "meters_encode": meters_encode,
        "meters_playout": meters_playout,
    })
}
