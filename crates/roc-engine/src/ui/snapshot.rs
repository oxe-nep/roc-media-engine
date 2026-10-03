use chrono::Utc;
use roc_pipelines::ChannelStatus;
use serde::Serialize;
use serde_json::{json, Value};

use crate::orchestrator::Orchestrator;
use crate::ui::state::UiState;

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
        "hls_url": format!("/hls/{}/preview.m3u8", ch.id),
        "preview_epoch": ch.preview_epoch,
        "backend": "media_engine",
    }))
}

pub fn recording_json(orch: &Orchestrator, ui: &UiState, id: u32) -> Value {
    let ch = orch.channel(id).ok();
    let meta = ui.rec_meta(id);
    let recording = ch.as_ref().map(|c| c.recording).unwrap_or(false);
    let mut elapsed = 0.0;
    let mut started_at: Option<String> = None;
    if recording {
        if let Some(t) = meta.started_at {
            elapsed = (Utc::now() - t).num_milliseconds() as f64 / 1000.0;
            started_at = Some(t.to_rfc3339());
        }
    }
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
        })
    });
    let name = if meta.name.is_empty() {
        ch.as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("Channel {id}"))
    } else {
        meta.name
    };
    let file_path = ch.as_ref().and_then(|c| c.recording_path.clone());
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

pub fn playout_json(orch: &Orchestrator) -> Vec<Value> {
    let mut out = Vec::new();
    for p in orch.list_playout() {
        let num_id = p
            .id
            .strip_prefix("decode-")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let running = matches!(
            p.status,
            ChannelStatus::Running | ChannelStatus::Waiting
        );
        let source = if p.source.as_deref().unwrap_or("").starts_with("srt://") {
            "srt"
        } else if p.source.is_some() {
            "file"
        } else {
            "srt"
        };
        out.push(json!({
            "id": num_id,
            "name": p.name,
            "status": if running { status_str(p.status) } else { "stopped" },
            "device": p.device,
            "device_label": p.device,
            "format_code": "Hp50",
            "decklink_out": true,
            "fixed": true,
            "source": source,
            "loop": false,
            "mode": "caller",
            "port": 0,
            "target": p.source.clone().unwrap_or_default(),
            "has_passphrase": false,
            "latency_ms": 120,
            "sending": running,
            "reconnects": 0,
            "error": p.last_error.unwrap_or_default(),
        }));
    }
    out.sort_by_key(|v| v.get("id").and_then(|x| x.as_u64()).unwrap_or(0));
    out
}

pub fn meters_maps(orch: &Orchestrator) -> (serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
    let mut enc = serde_json::Map::new();
    let mut play = serde_json::Map::new();
    for ch in orch.list_channels() {
        let m = from_peaks(ch.audio_peaks.as_deref());
        let v = serde_json::to_value(m).unwrap_or(json!({}));
        enc.insert(ch.id.to_string(), v.clone());
        play.insert(ch.id.to_string(), json!({"l": -90.0, "r": -90.0, "channels": silence_peaks()}));
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
        "playout": playout_json(orch),
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
