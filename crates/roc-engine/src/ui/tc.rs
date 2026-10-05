//! TC burn-in API helpers (release pair, build launch opts, JSON).

use anyhow::{bail, Context, Result};
use roc_pipelines::{
    TcLoopLaunchOpts, TcLoopPosition, TcLoopSnapshot, TcLoopSource, TcLoopStatus,
};
use serde_json::{json, Value};

use crate::orchestrator::Orchestrator;
use crate::ui::state::{TcMeta, UiState};

pub fn effective_udp_port(meta: &TcMeta, id: u32) -> u16 {
    meta.effective_udp_port(id)
}

pub fn tc_info_json(id: u32, meta: &TcMeta, live: Option<&TcLoopSnapshot>) -> Value {
    let udp = effective_udp_port(meta, id);
    let (x, y) = meta.resolved_xy();
    if let Some(s) = live {
        return json!({
            "id": id,
            "enabled": s.enabled,
            "status": match s.status {
                TcLoopStatus::Off => "off",
                TcLoopStatus::Running => "running",
                TcLoopStatus::Restarting => "restarting",
                TcLoopStatus::Error => "error",
            },
            "source": s.source.as_str(),
            "udp_port": s.udp_port,
            "fontsize": s.fontsize,
            "opacity": s.opacity,
            "position": s.position.as_str(),
            "x": s.x,
            "y": s.y,
            "error": s.error,
            "timecode": s.timecode,
        });
    }
    json!({
        "id": id,
        "enabled": meta.enabled,
        "status": if meta.enabled { "error" } else { "off" },
        "source": meta.source,
        "udp_port": udp,
        "fontsize": meta.fontsize,
        "opacity": meta.opacity,
        "position": meta.position,
        "x": x,
        "y": y,
        "error": if meta.enabled { Value::String("TC not running".into()) } else { Value::Null },
        "timecode": Value::Null,
    })
}

/// Stop encode / playout / SRT so DeckLink can be claimed by TC.
pub fn release_channel_pair(orch: &Orchestrator, ui: &UiState, id: u32) {
    let client_id = format!("decode-{id}");
    let _ = orch.stop_tc_loop(id);
    let _ = orch.stop_srt(id);
    let _ = orch.stop_proxy_recording(id);
    let _ = orch.stop_hq_recording(id);
    ui.mark_recording_stopped_role(id, roc_pipelines::RecordingRole::Proxy);
    ui.mark_recording_stopped_role(id, roc_pipelines::RecordingRole::Hq);
    let _ = orch.stop_capture(id);
    let _ = orch.stop_playout(&client_id);
    // Brief DeckLink settle (Go uses 3s; keep shorter for UX).
    std::thread::sleep(std::time::Duration::from_millis(800));
}

pub fn build_launch_opts(
    orch: &Orchestrator,
    id: u32,
    meta: &TcMeta,
    hls_dir: Option<String>,
) -> Result<TcLoopLaunchOpts> {
    let ch = orch
        .channel_config(id)
        .with_context(|| format!("encode channel {id} not in config"))?;
    let client_id = format!("decode-{id}");
    let play = orch
        .playout_config(&client_id)
        .with_context(|| format!("playout {client_id} not in config"))?;

    let snap = orch.list_channels().into_iter().find(|c| c.id == id);

    if ch.device.trim().is_empty() {
        bail!("encode channel {id} has no DeckLink device");
    }
    if play.device.trim().is_empty() {
        bail!("decode {id} has no DeckLink device");
    }

    // Prefer live probe so OUT/SRT match the actual IN (locked_mode is cleared when
    // encode stops for exclusive TC). Fall back to last lock / config / 1080i50.
    let input_mode = match roc_pipelines::probe_input_format(&ch.device, 4000) {
        Ok(fmt) => {
            tracing::info!(
                channel = id,
                mode = %fmt.mode,
                format = %fmt.summary(),
                "TC probed input format"
            );
            fmt.mode
        }
        Err(err) => {
            let fallback = snap
                .as_ref()
                .and_then(|s| s.locked_mode.clone())
                .filter(|m| !m.is_empty() && m != "auto")
                .or_else(|| {
                    ch.mode
                        .clone()
                        .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("auto"))
                })
                .unwrap_or_else(|| "1080i50".into());
            tracing::warn!(
                channel = id,
                error = %err,
                mode = %fallback,
                "TC input probe failed — using fallback mode"
            );
            fallback
        }
    };

    // OUT must match IN (1080i in → 1080i DeckLink + SRT).
    let output_mode = input_mode.clone();

    let preset_id = snap
        .as_ref()
        .map(|s| s.encode_preset.as_str())
        .or(ch.encode_preset.as_deref())
        .unwrap_or("");
    let preset = orch
        .list_presets()
        .into_iter()
        .find(|(pid, _)| !preset_id.is_empty() && pid == preset_id)
        .or_else(|| orch.list_presets().into_iter().next())
        .map(|(_, p)| p)
        .context("no encode/proxy preset configured")?;

    let (x, y) = meta.resolved_xy();
    Ok(TcLoopLaunchOpts {
        input_device: ch.device.clone(),
        output_device: play.device.clone(),
        input_mode,
        output_mode,
        source: TcLoopSource::parse(&meta.source),
        udp_port: effective_udp_port(meta, id),
        fontsize: meta.fontsize.clamp(12, 200),
        opacity: meta.opacity.clamp(0.15, 1.0),
        position: TcLoopPosition::nearest(x, y),
        x,
        y,
        preset,
        hls_dir,
    })
}

pub fn start_tc(orch: &Orchestrator, ui: &UiState, id: u32, hls_base: &str) -> Result<Value> {
    release_channel_pair(orch, ui, id);
    let mut meta = ui.tc(id);
    meta.enabled = true;
    ui.set_tc(id, meta.clone());
    ui.set_workflow_mode(id, "tc");

    let dir = format!("{hls_base}/playout/{id}");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let name = ent.file_name();
            let n = name.to_string_lossy();
            if n.ends_with(".m3u8")
                || n.ends_with(".ts")
                || n.starts_with("thumb")
                || n.starts_with("pv")
            {
                let _ = std::fs::remove_file(ent.path());
            }
        }
    }

    let opts = build_launch_opts(orch, id, &meta, Some(dir))?;
    match orch.start_tc_loop(id, opts) {
        Ok(snap) => Ok(tc_info_json(id, &meta, Some(&snap))),
        Err(e) => {
            let failed = meta.clone();
            // Keep enabled=true so UI shows error state until STOP.
            ui.set_tc(id, failed.clone());
            Ok(json!({
                "id": id,
                "enabled": true,
                "status": "error",
                "source": failed.source,
                "udp_port": effective_udp_port(&failed, id),
                "fontsize": failed.fontsize,
                "opacity": failed.opacity,
                "position": failed.position,
                "error": e.to_string(),
                "timecode": Value::Null,
            }))
        }
    }
}

pub fn stop_tc(orch: &Orchestrator, ui: &UiState, id: u32) -> Result<Value> {
    let _ = orch.stop_tc_loop(id);
    let mut meta = ui.tc(id);
    meta.enabled = false;
    ui.set_tc(id, meta.clone());
    Ok(tc_info_json(id, &meta, None))
}
