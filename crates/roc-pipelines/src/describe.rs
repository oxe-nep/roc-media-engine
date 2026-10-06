//! Human-readable / gst-launch-1.0 compatible pipeline strings.

use roc_config::EncodePreset;

use crate::parse_bitrate;

#[derive(Debug, Clone)]
pub struct CaptureLaunchOpts {
    pub device: String,
    /// GStreamer DeckLink mode enum name, e.g. `1080p50`, `1080i50`, `auto`.
    pub mode: String,
    pub preset: EncodePreset,
    pub preview_path: Option<String>,
    pub record_path: Option<String>,
    pub srt_url: Option<String>,
    pub udp_egress: Option<String>,
    /// When true, include a `tee` with encode + preview branches (Fas 0 spike).
    pub with_tee_preview: bool,
}

#[derive(Debug, Clone)]
pub struct PlayoutLaunchOpts {
    pub source: String,
    pub device: String,
    pub format_code: String,
    /// When set, write `listen_0.m3u8` + JPEG thumb under this directory.
    pub hls_dir: Option<String>,
    /// Stereo audio track count to map onto DeckLink (1..=4). Matches encode REC pairs.
    pub audio_pairs: usize,
    /// True when tracks are AAC/MPEG; false for raw PCM (MXF mezz).
    pub audio_compressed: bool,
}

/// DeckLink IN → TC overlay → DeckLink OUT + proxy encode (SRT/WebRTC/JPEG).
#[derive(Debug, Clone)]
pub struct TcLoopLaunchOpts {
    pub input_device: String,
    pub output_device: String,
    /// GStreamer mode for `decklinkvideosrc` (e.g. `1080p50`).
    pub input_mode: String,
    /// GStreamer mode for `decklinkvideosink` (e.g. `1080p50`).
    pub output_mode: String,
    /// `tod` uses `clockoverlay`; `external` uses `textoverlay name=tc_text`.
    pub source: crate::TcLoopSource,
    /// UDP listen port when `source == External` (default 9300+id).
    pub udp_port: u16,
    pub fontsize: u32,
    pub opacity: f64,
    pub position: crate::TcLoopPosition,
    /// Normalized X/Y of text top-left (0..=1). Used with absolute overlay alignment.
    pub x: f64,
    pub y: f64,
    /// Proxy encode preset (NVENC → MPEG-TS for SRT / WebRTC).
    pub preset: EncodePreset,
    pub hls_dir: Option<String>,
}

/// Map `"DeckLink IP 100G (1)"` / `"1"` / `"0"` → DeckLink `device-number` (0-based).
pub fn decklink_device_number(device: &str) -> u32 {
    let s = device.trim();
    if let Ok(n) = s.parse::<u32>() {
        // Treat bare 1..=8 as display index; 0 stays 0.
        return if (1..=32).contains(&n) { n - 1 } else { n };
    }
    if let Some(start) = s.rfind('(') {
        if let Some(end) = s.rfind(')') {
            if end > start + 1 {
                if let Ok(n) = s[start + 1..end].parse::<u32>() {
                    if n >= 1 {
                        return n - 1;
                    }
                }
            }
        }
    }
    0
}

fn decklink_src(device: &str, mode: &str) -> String {
    // Prefer a concrete mode (caller probes when config is `auto`).
    // drop-no-signal-frames keeps the pipeline alive across brief ST 2110 gaps.
    // profile=one-sub-device-full allows simultaneous IN (encode) + OUT (decode)
    // on the same DeckLink IP sub-device.
    let mode = if mode.trim().is_empty() || mode.eq_ignore_ascii_case("auto") {
        "1080p50" // last-resort fallback; prefer probe_input_format first
    } else {
        mode.trim()
    };
    format!(
        "decklinkvideosrc name=dlsrc device-number={} mode={} drop-no-signal-frames=true profile=one-sub-device-full",
        decklink_device_number(device),
        mode
    )
}

/// Parse `udp://239.255.28.1:21001?...` → (host, port).
fn udp_host_port(url: &str) -> (String, u32) {
    let rest = url
        .strip_prefix("udp://")
        .or_else(|| url.strip_prefix("UDP://"))
        .unwrap_or(url);
    let authority = rest.split(['?', '/']).next().unwrap_or(rest);
    if let Some((host, port)) = authority.rsplit_once(':') {
        if let Ok(p) = port.parse::<u32>() {
            return (host.to_string(), p);
        }
    }
    ("239.255.28.1".into(), 21001)
}

/// How many AAC stereo pairs to mux for a preset (`2` → 1 pair, `8` → 4 pairs).
/// MPEG-TS cannot carry 8ch PCM; FFmpeg/Go use the same 4×AAC layout.
pub fn aac_stereo_pairs(audio_channels: u32) -> usize {
    if audio_channels >= 8 {
        4
    } else {
        1
    }
}

/// 8→2 matrix selecting stereo pair `pair` (0=ch1-2 … 3=ch7-8).
pub fn stereo_pair_matrix(pair: usize) -> String {
    let mut rows = Vec::with_capacity(2);
    for out_ch in 0..2 {
        let mut coeffs = Vec::with_capacity(8);
        for in_ch in 0..8 {
            let v = if in_ch == pair * 2 + out_ch {
                "1.0"
            } else {
                "0.0"
            };
            coeffs.push(v.to_string());
        }
        rows.push(format!("<{}>", coeffs.join(", ")));
    }
    format!("<{}>", rows.join(", "))
}

/// 2→8 matrix placing a stereo pair into DeckLink ch 1-2 … 7-8 (inverse of encode).
pub fn stereo_into_8ch_matrix(pair: usize) -> String {
    let mut rows = Vec::with_capacity(8);
    for out_ch in 0..8 {
        let mut coeffs = Vec::with_capacity(2);
        for in_ch in 0..2 {
            let v = if out_ch == pair * 2 + in_ch {
                "1.0"
            } else {
                "0.0"
            };
            coeffs.push(v.to_string());
        }
        rows.push(format!("<{}>", coeffs.join(", ")));
    }
    format!("<{}>", rows.join(", "))
}

/// One AAC encode fan-out target: named mpegtsmux request pad + optional valve.
struct AacMuxOut<'a> {
    mux_name: &'a str,
    /// When set, inserts `valve name={prefix}{pair} drop=true` (SRT PMT gate).
    valve_prefix: Option<&'a str>,
}

/// Program AAC into muxes (and optionally listen HLS) — encode once per pair.
///
/// Sharing SRT+UDP+listen avoids ~12 `voaacenc`/channel (CPU overload → crackle).
fn mpegts_program_aac(
    name_prefix: &str,
    aac_bps: u64,
    pairs: usize,
    outputs: &[AacMuxOut<'_>],
    feed_listen: bool,
) -> String {
    let pairs = pairs.max(1);
    if outputs.is_empty() && !feed_listen {
        return String::new();
    }
    // Deep non-leaky queues on the live encode path. Leaky only on sinks that
    // may stall (HLS disk, SRT PMT hold) so they cannot backpressure voaacenc.
    let q = "queue max-size-buffers=0 max-size-bytes=0 max-size-time=200000000";
    let aac_caps = "audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved";
    if pairs == 1 && outputs.len() == 1 && !feed_listen {
        let matrix = stereo_pair_matrix(0);
        let out = &outputs[0];
        let valve = out
            .valve_prefix
            .map(|p| format!("valve name={p}0 drop=true ! "))
            .unwrap_or_default();
        return format!(
            "a. ! {q} ! \
             audioconvert mix-matrix=\"{matrix}\" ! {aac_caps} ! \
             voaacenc bitrate={aac_bps} ! aacparse ! \
             capsfilter caps=audio/mpeg,mpegversion=4,stream-format=adts ! \
             {valve}{mux}.",
            mux = out.mux_name,
        );
    }
    let raw_tee = format!("{name_prefix}_a_tee");
    let mut parts = vec![format!("a. ! {q} ! tee name={raw_tee}")];
    for pair in 0..pairs {
        let matrix = stereo_pair_matrix(pair);
        let aac_tee = format!("{name_prefix}_aac{pair}");
        parts.push(format!(
            "{raw_tee}. ! {q} ! \
             audioconvert mix-matrix=\"{matrix}\" ! {aac_caps} ! \
             voaacenc bitrate={aac_bps} ! aacparse ! \
             capsfilter caps=audio/mpeg,mpegversion=4,stream-format=adts ! \
             tee name={aac_tee}"
        ));
        for out in outputs {
            // Non-leaky into both muxes so all AAC pads stay in the PMT.
            // SRT egress valve starts open (see capture arm) so this cannot
            // backpressure the shared tee into UDP/listen.
            parts.push(format!(
                "{aac_tee}. ! {q} ! {mux}.",
                mux = out.mux_name,
            ));
        }
        if feed_listen {
            // Deep leaky queue: short queues dropped AAC under CPU load → crackle
            // in listen/preview. Prefer ~500ms before leaking.
            parts.push(format!(
                "{aac_tee}. ! queue max-size-buffers=0 max-size-bytes=0 \
                 max-size-time=500000000 leaky=downstream ! hls_l{pair}.audio"
            ));
        }
    }
    parts.join(" ")
}

/// Bus-message peak meters for the 8ch audio tee (`level` → Element "level").
/// ~30 Hz for snappy LED meters without flooding the UI WebSocket.
fn meter_branch() -> &'static str {
    "a. ! queue max-size-buffers=8 leaky=downstream ! \
     level name=ameter interval=33000000 post-messages=true ! \
     fakesink sync=false async=false"
}

/// 1 fps JPEG thumbnail for the encode grid (`/thumb/{id}`).
fn thumb_jpeg_branch(hls_dir: &str) -> String {
    // max-files=1 + %05d keeps a single rolling frame; API serves newest match.
    let loc = format!("{hls_dir}/thumb%05d.jpg");
    format!(
        "t. ! queue max-size-buffers=2 leaky=downstream ! \
         videoconvert ! videoscale ! videorate skip-to-first=true ! \
         video/x-raw,width=640,height=360,framerate=1/1 ! \
         jpegenc quality=80 idct-method=float ! \
         multifilesink location=\"{loc}\" max-files=1 next-file=buffer \
         post-messages=false sync=false async=false"
    )
}

fn hls_dir_from_playlist(playlist: &str) -> String {
    std::path::Path::new(playlist)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

fn hls_generation() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn encode_family(video_codec: &str) -> EncodeFamily {
    let c = video_codec.to_ascii_lowercase();
    if c.contains("265") || c.contains("hevc") {
        EncodeFamily::Hevc
    } else {
        EncodeFamily::H264
    }
}

/// Live/SRT always stays on NVENC. Mezz presets only affect the REC branch.
fn live_nvenc_codec(preset: &EncodePreset) -> &str {
    if roc_config::is_mezz_codec(&preset.video_codec) {
        "nvh264enc"
    } else {
        &preset.video_codec
    }
}

fn live_nvenc_bitrate_kbit(preset: &EncodePreset) -> u64 {
    if roc_config::is_mezz_codec(&preset.video_codec) {
        // Fixed proxy for SRT/UDP while mezz encodes on the raw tee.
        12_000
    } else {
        parse_bitrate(&preset.video_bitrate)
            .map(|b| b / 1000)
            .unwrap_or(12_000)
    }
}

fn live_nvenc_preset(preset: &EncodePreset) -> &str {
    if roc_config::is_mezz_codec(&preset.video_codec) {
        "low-latency-hq"
    } else {
        &preset.video_preset
    }
}

fn live_nvenc_gop(preset: &EncodePreset) -> u32 {
    if roc_config::is_mezz_codec(&preset.video_codec) {
        50
    } else {
        preset.video_gop
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EncodeFamily {
    H264,
    Hevc,
}

impl EncodeFamily {
    fn parse_element(self) -> &'static str {
        match self {
            Self::H264 => "h264parse",
            Self::Hevc => "h265parse",
        }
    }

    fn encoder_element(self) -> &'static str {
        match self {
            Self::H264 => "nvh264enc",
            Self::Hevc => "nvh265enc",
        }
    }

    /// Caps name for Annex-B (in-band VPS/SPS/PPS). Required for MPEG-TS / mid-join.
    fn byte_stream_caps(self) -> &'static str {
        match self {
            Self::H264 => "video/x-h264,stream-format=byte-stream,alignment=au",
            Self::Hevc => "video/x-h265,stream-format=byte-stream,alignment=au",
        }
    }
}

/// NVENC encode chain (H.264 or H.265) via NV12 + cudaupload.
///
/// Flags mirror FFmpeg capture (`-bf 0`, `-forced-idr`, mid-join friendly):
/// force `byte-stream` so `repeat-sequence-header` is honored (ignored for avc/hvc1)
/// and MPEG-TS players get in-band parameter sets on every IDR.
fn nvenc_chain(video_codec: &str, preset: &str, bitrate_kbit: u64, gop: u32) -> String {
    nvenc_chain_ex(video_codec, preset, bitrate_kbit, gop, false)
}

/// `interlaced`: keep field structure for TC burn-in (1080i in → 1080i SRT).
/// Note: older `nvh264enc` builds have no `interlaced-encoding` property — we rely on
/// feeding interleaved frames and DeckLink OUT caps instead.
fn nvenc_chain_ex(
    video_codec: &str,
    preset: &str,
    bitrate_kbit: u64,
    gop: u32,
    interlaced: bool,
) -> String {
    let family = encode_family(video_codec);
    let enc = family.encoder_element();
    let parse = family.parse_element();
    let caps = family.byte_stream_caps();
    // nvh265enc has no `bframes` property; H.264 needs an explicit 0 (hq defaults to B-frames).
    let bframes = match family {
        EncodeFamily::H264 => " bframes=0",
        EncodeFamily::Hevc => "",
    };
    let _ = interlaced; // reserved: property not available on all NVENC builds
    format!(
        "videoconvert ! video/x-raw,format=NV12 ! cudaupload ! \
         {enc} preset={preset} bitrate={bitrate_kbit} gop-size={gop} \
         zerolatency=true aud=true repeat-sequence-header=true{bframes} ! \
         {caps} ! {parse} config-interval=-1"
    )
}

/// True when a DeckLink/GST mode or format summary looks interlaced (e.g. `1080i50`).
pub fn gst_mode_looks_interlaced(mode: &str) -> bool {
    let m = mode.to_ascii_lowercase();
    // Match scan letter after height digits: 1080i50, 1920x1080i50.
    let bytes = m.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'i' {
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| bytes.get(j)).copied();
        let next = bytes.get(i + 1).copied();
        let prev_digit = prev.map(|c| c.is_ascii_digit()).unwrap_or(false);
        let next_digit_or_end = next.map(|c| c.is_ascii_digit()).unwrap_or(true);
        if prev_digit && next_digit_or_end {
            return true;
        }
    }
    false
}

fn gst_mode_is_interlaced(mode: &str) -> bool {
    gst_mode_looks_interlaced(mode)
}

fn decklink_video_sink(device: &str, mode: &str) -> String {
    format!(
        "decklinkvideosink device-number={} mode={} profile=one-sub-device-full sync=true",
        decklink_device_number(device),
        mode
    )
}

fn decklink_audio_sink(device: &str) -> String {
    format!(
        "decklinkaudiosink device-number={}",
        decklink_device_number(device)
    )
}

/// Spike: DeckLink → deinterlace → NVENC (h264|hevc) → optional tee → file (+ preview).
pub fn build_spike_tee_launch(device: &str, output_mp4: &str, with_preview: bool) -> String {
    build_spike_tee_launch_codec(device, output_mp4, with_preview, "nvh264enc")
}

pub fn build_spike_tee_launch_codec(
    device: &str,
    output_mp4: &str,
    with_preview: bool,
    video_codec: &str,
) -> String {
    build_spike_tee_launch_codec_mode(device, output_mp4, with_preview, video_codec, "1080p50")
}

pub fn build_spike_tee_launch_codec_mode(
    device: &str,
    output_mp4: &str,
    with_preview: bool,
    video_codec: &str,
    mode: &str,
) -> String {
    let src = decklink_src(device, mode);
    let enc = nvenc_chain(video_codec, "low-latency-hq", 12_000, 50);
    if with_preview {
        format!(
            "{src} ! \
             deinterlace ! tee name=t \
             t. ! queue ! {enc} ! mp4mux fragment-duration=1000 ! filesink location=\"{output_mp4}\" \
             t. ! queue ! videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 ! \
             h264parse ! mpegtsmux ! filesink location=\"{output_mp4}.preview.ts\""
        )
    } else {
        format!(
            "{src} ! \
             deinterlace ! {enc} ! mp4mux fragment-duration=1000 ! filesink location=\"{output_mp4}\""
        )
    }
}

pub fn build_capture_launch(opts: &CaptureLaunchOpts) -> String {
    let bitrate_kbit = parse_bitrate(&opts.preset.video_bitrate)
        .map(|b| b / 1000)
        .unwrap_or(12_000);
    let gop = opts.preset.video_gop;
    let preset = &opts.preset.video_preset;
    let enc = nvenc_chain(&opts.preset.video_codec, preset, bitrate_kbit, gop);
    let src = decklink_src(&opts.device, &opts.mode);

    let mut branches = Vec::new();

    if let Some(path) = &opts.record_path {
        branches.push(format!(
            "t. ! queue name=q_rec ! {enc} ! mp4mux fragment-duration=1000 ! filesink location=\"{path}\" sync=false"
        ));
    }

    if let Some(url) = &opts.srt_url {
        branches.push(format!(
            "t. ! queue name=q_srt ! {enc} ! mpegtsmux alignment=7 ! srtsink uri=\"{url}\" wait-for-connection=false"
        ));
    }

    if opts.udp_egress.is_some() {
        let url = opts.udp_egress.as_deref().unwrap_or("");
        let (host, port) = udp_host_port(url);
        branches.push(format!(
            "t. ! queue name=q_udp ! {enc} ! mpegtsmux alignment=7 ! udpsink host={host} port={port} sync=false async=false"
        ));
    }

    if opts.with_tee_preview || opts.preview_path.is_some() {
        let playlist = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview/preview.m3u8".into());
        let seg = {
            let path = std::path::Path::new(&playlist);
            let dir = path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into());
            let gen = hls_generation();
            format!("{dir}/pv{gen}_%05d.ts")
        };
        // CFR 10 fps (no drop-only): drop-only + hlssink2 produced EXTINF ~0.1/1.0
        // alternation that made hls.js hang. Match FFmpeg `fps=10`.
        branches.push(format!(
            "t. ! queue max-size-buffers=3 leaky=downstream ! \
             videoconvert ! videoscale ! videorate ! \
             video/x-raw,width=640,height=360,framerate=10/1 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 key-int-max=10 bframes=0 ! \
             video/x-h264,profile=baseline ! h264parse config-interval=-1 ! \
             hlssink2 location=\"{seg}\" playlist-location=\"{playlist}\" \
             target-duration=1 max-files=6 playlist-length=6"
        ));
    }

    branches.push("t. ! queue name=q_idle leaky=downstream ! fakesink sync=false".into());

    format!(
        "{src} ! \
         deinterlace mode=auto ! \
         tee name=t \
         {branches}",
        branches = branches.join(" ")
    )
}

/// Optimized capture: encode once (NVENC), then tee the bitstream to SRT / UDP.
///
/// Graph:
/// `decklink → tee raw → (mezz hot-attach) | deinterlace → tee t → NVENC / thumb`
///
/// Mezz REC (DNxHD) attaches on **`raw`** so interlaced sources keep fields.
/// Proxy/SRT/JPEG use progressive **`t`**.
pub fn build_capture_encode_once_launch(opts: &CaptureLaunchOpts) -> String {
    let live_codec = live_nvenc_codec(&opts.preset);
    let bitrate_kbit = live_nvenc_bitrate_kbit(&opts.preset);
    let gop = live_nvenc_gop(&opts.preset);
    let preset = live_nvenc_preset(&opts.preset);
    let family = encode_family(live_codec);
    let parse = family.parse_element();
    let src = decklink_src(&opts.device, &opts.mode);

    let mut out_branches = Vec::new();
    if let Some(path) = &opts.record_path {
        out_branches.push(format!(
            "e. ! queue ! {parse} ! mp4mux fragment-duration=1000 ! filesink location=\"{path}\" sync=false"
        ));
    }
    let aac_bps = parse_bitrate(&opts.preset.audio_bitrate)
        .unwrap_or(192_000)
        .clamp(64_000, 320_000);
    let pairs = aac_stereo_pairs(opts.preset.audio_channels);

    // MPEG-TS egress follows HydraSRT's program model (streamband/hydra-srt):
    // one mux → tee → destinations. Do NOT remux again for SRT — a second
    // mpegtsmux races A/V and emits a poison first PMT that MediaMTX locks on.
    // Docs: https://gstreamer.freedesktop.org/documentation/mpegtsmux/mpegtsmux.html
    // (alignment=7 for UDP/SRT packetization; no property waits for both pads).
    //
    // Always expose `appsink srt_in` on the TS tee when UDP (or SRT-only) is
    // configured. Rust arms appsrc→srtsink on start_srt / tears it down on
    // stop_srt — no capture relaunch, so REC on tee `e` keeps running.
    let bs = family.byte_stream_caps();
    let has_udp = opts.udp_egress.is_some();
    let has_ts_egress = has_udp || opts.srt_url.is_some();
    let srt_appsink = "queue name=q_srt max-size-buffers=8 max-size-time=0 max-size-bytes=0 \
         leaky=downstream ! \
         appsink name=srt_in emit-signals=true sync=false async=false \
         max-buffers=8 drop=true";
    match &opts.udp_egress {
        Some(udp) => {
            let (host, port) = udp_host_port(udp);
            out_branches.push(format!(
                "e. ! queue ! {parse} config-interval=-1 ! {bs} ! \
                 mpegtsmux name=tsmux alignment=7 ! tee name=ts_out allow-not-linked=true \
                 ts_out. ! queue ! udpsink host={host} port={port} sync=false async=false \
                 ts_out. ! {srt_appsink}"
            ));
        }
        None if opts.srt_url.is_some() => {
            out_branches.push(format!(
                "e. ! queue ! {parse} config-interval=-1 ! {bs} ! \
                 mpegtsmux name=tsmux alignment=7 ! {srt_appsink}"
            ));
        }
        None => {}
    }
    out_branches.push("e. ! queue leaky=downstream ! fakesink sync=false".into());

    // Grid preview is 1 fps JPEG only (no always-on HLS). Listen A/V attaches
    // on demand when the preview modal opens.
    let thumb = if opts.with_tee_preview || opts.preview_path.is_some() {
        let playlist = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview/preview.m3u8".into());
        let dir = hls_dir_from_playlist(&playlist);
        thumb_jpeg_branch(&dir)
    } else {
        String::new()
    };

    let mut aac_parts = Vec::new();
    if has_ts_egress {
        // One program AAC into the single tsmux (Hydra: MediaLane::Program).
        aac_parts.push(mpegts_program_aac(
            "prog",
            aac_bps,
            pairs,
            &[AacMuxOut {
                mux_name: "tsmux",
                valve_prefix: None,
            }],
            false, // no always-on listen HLS pads
        ));
    }
    let shared_aac = aac_parts.join(" ");

    let need_audio = opts.with_tee_preview
        || opts.preview_path.is_some()
        || opts.udp_egress.is_some()
        || opts.srt_url.is_some();
    let (audio_src, meter) = if need_audio {
        let num = decklink_device_number(&opts.device);
        (
            format!(
                "decklinkaudiosrc device-number={num} channels=8 ! \
                 audioconvert ! audio/x-raw,channels=8,rate=48000,layout=interleaved ! tee name=a"
            ),
            meter_branch().to_string(),
        )
    } else {
        (String::new(), String::new())
    };

    format!(
        "{src} ! \
         tee name=raw allow-not-linked=true \
         raw. ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
           deinterlace mode=auto ! tee name=t \
         {audio_src} \
         t. ! queue ! {enc} ! \
         tee name=e \
         {out_branches} \
         {thumb} \
         {shared_aac} \
         {meter}",
        enc = nvenc_chain(live_codec, preset, bitrate_kbit, gop),
        out_branches = out_branches.join(" "),
    )
}

/// Decode one stereo pair from demux → 48 kHz S16LE stereo.
fn playout_stereo_decode(compressed: bool) -> &'static str {
    if compressed {
        "audio/mpeg ! aacparse ! avdec_aac ! audioconvert ! audioresample"
    } else {
        "audio/x-raw ! audioconvert ! audioresample"
    }
}

/// File/SRT audio → DeckLink 8ch using the inverse of encode's stereo-pair matrices.
///
/// Each demux AAC/PCM track is expanded into its pair slots (1-2 … 7-8) and mixed.
/// File demux uses named pads (`audio_0`…) so track order matches encode pair order.
fn build_playout_audio(pairs: usize, compressed: bool, asink: &str, named_pads: bool) -> String {
    let pairs = pairs.clamp(1, 4);
    let dec = playout_stereo_decode(compressed);
    let q = "queue max-size-buffers=0 max-size-bytes=0 max-size-time=0";
    let stereo = "audio/x-raw,format=S16LE,channels=2,rate=48000,layout=interleaved";
    let out8 = "audio/x-raw,format=S16LE,channels=8,rate=48000,layout=interleaved";
    let meter = format!(
        "a. ! queue max-size-buffers=8 leaky=downstream ! \
           level name=ameter interval=33000000 post-messages=true ! \
           fakesink sync=false async=false"
    );
    let pad = |n: usize| -> String {
        if named_pads {
            format!("d.audio_{n}")
        } else {
            "d.".into()
        }
    };

    if pairs == 1 {
        // One track: keep on ch1-2 (encode pair 0); leave 3-8 silent.
        let matrix = stereo_into_8ch_matrix(0);
        let src = pad(0);
        return format!(
            "{src} ! {q} ! {dec} ! audioconvert ! audioresample ! {stereo} ! \
             audioconvert mix-matrix=\"{matrix}\" ! {out8} ! tee name=a \
             a. ! {q} ! {asink} \
             {meter}"
        );
    }

    let mut parts = Vec::with_capacity(pairs + 2);
    parts.push("audiomixer name=mix start-time-selection=first".to_string());
    for n in 0..pairs {
        let matrix = stereo_into_8ch_matrix(n);
        let src = pad(n);
        parts.push(format!(
            "{src} ! {q} ! {dec} ! audioconvert ! audioresample ! {stereo} ! \
             audioconvert mix-matrix=\"{matrix}\" ! {out8} ! mix."
        ));
    }
    parts.push(format!(
        "mix. ! audioconvert ! {out8} ! tee name=a \
         a. ! {q} ! {asink} \
         {meter}"
    ));
    parts.join(" ")
}

pub fn build_playout_launch(opts: &PlayoutLaunchOpts) -> String {
    let src = if opts.source.starts_with("srt://") {
        format!("srtsrc uri=\"{}\" ! tsdemux name=d", opts.source)
    } else if opts.source.ends_with(".ts") {
        format!("filesrc location=\"{}\" ! tsdemux name=d", opts.source)
    } else {
        format!("filesrc location=\"{}\" ! qtdemux name=d", opts.source)
    };

    // GST mode enums use names like 1080p50, not BMD Hp50 — map common codes.
    // Encode path deinterlaces to progressive; prefer progressive sink modes so
    // DeckLink OUT actually shows frames (Hi50 + progressive decode → black video).
    let mode = match opts.format_code.as_str() {
        "auto" | "" => "1080p50",
        "Hp50" | "hp50" | "1080p50" => "1080p50",
        // Map interlaced UI codes to progressive equivalents for our progressive encode.
        "Hi50" | "hi50" | "1080i50" => "1080p50",
        "Hp25" | "hp25" => "1080p25",
        "Hp59.94" | "Hp5994" | "1080p5994" => "1080p5994",
        "Hi59.94" | "Hi5994" | "1080i5994" => "1080p5994",
        other => other,
    };

    // Progressive vs interlaced sink geometry for forced caps.
    // Our encode path is progressive; avoid forcing interleaved caps (breaks negotiation).
    let (height, fr) = match mode {
        "1080i50" | "1080p25" => ("1080", "25/1"),
        "1080i5994" => ("1080", "30000/1001"),
        "1080p5994" => ("1080", "60000/1001"),
        _ => ("1080", "50/1"),
    };

    let preview = if let Some(dir) = &opts.hls_dir {
        let _ = std::fs::create_dir_all(dir);
        let seg = format!("{dir}/pv%05d.ts");
        let playlist = format!("{dir}/listen_0.m3u8");
        let thumb = format!("{dir}/thumb%05d.jpg");
        // Video-only HLS for the decode grid (UI rewrites preview → listen_0).
        // Stereo listen audio can rejoin once pad negotiation is proven stable.
        format!(
            "v. ! queue max-size-buffers=8 leaky=downstream ! \
               videoconvert ! videoscale ! videorate skip-to-first=true ! \
               video/x-raw,width=640,height=360,framerate=10/1 ! \
               x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 key-int-max=20 bframes=0 ! \
               video/x-h264,profile=baseline ! h264parse config-interval=-1 ! \
               hlssink2 name=hls_l0 location=\"{seg}\" playlist-location=\"{playlist}\" \
               target-duration=1 max-files=6 playlist-length=6 \
             v. ! queue max-size-buffers=2 leaky=downstream ! \
               videoconvert ! videoscale ! videorate skip-to-first=true ! \
               video/x-raw,width=640,height=360,framerate=1/1 ! \
               jpegenc quality=80 idct-method=float ! \
               multifilesink location=\"{thumb}\" max-files=1 next-file=buffer \
               post-messages=false sync=false async=false"
        )
    } else {
        String::new()
    };

    // Caps on demux pads are required: our MPEG-TS often exposes AAC before H.264,
    // so untyped `d.` can latch the audio pad onto the video branch → audio-only OUT.
    let asink = decklink_audio_sink(&opts.device);
    // qtdemux exposes stable `audio_N` pads; tsdemux does not — use anonymous there.
    let named_audio = !opts.source.starts_with("srt://") && !opts.source.ends_with(".ts");
    let audio = build_playout_audio(opts.audio_pairs, opts.audio_compressed, &asink, named_audio);
    format!(
        "{src} \
         d. ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
           video/x-h264 ! h264parse config-interval=-1 ! avdec_h264 ! \
           videoconvert ! identity name=vpos silent=true ! tee name=v \
         v. ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
           videoscale ! videorate skip-to-first=true ! \
           video/x-raw,format=UYVY,width=1920,height={height},framerate={fr} ! \
           {vsink} \
         {audio} \
         {preview}",
        vsink = decklink_video_sink(&opts.device, mode),
    )
}

pub(crate) fn tc_overlay_color(opacity: f64) -> u32 {
    let a = ((opacity.clamp(0.15, 1.0) * 255.0).round() as u32).min(255);
    // AARRGGBB — white text
    (a << 24) | 0x00FF_FFFF
}

fn tc_overlay_element(opts: &TcLoopLaunchOpts) -> String {
    // Pixel size on the video frame (Pango `px`) — matches UI fontsize / FFmpeg drawtext.
    let font = opts.fontsize.clamp(12, 200);
    let x = opts.x.clamp(0.0, 1.0);
    let y = opts.y.clamp(0.0, 1.0);
    let color = tc_overlay_color(opts.opacity);
    let font_desc = format!("Sans Bold {font}px");
    // Absolute placement: xpos/ypos are 0..=1 of the frame (top-left of the text layout).
    // TOD: clockoverlay is always HH:MM:SS (no frames). External: textoverlay updated as HH:MM:SS.
    match opts.source {
        crate::TcLoopSource::Tod => format!(
            "clockoverlay name=tc_overlay font-desc=\"{font_desc}\" \
             halignment=absolute valignment=absolute xpos={x:.4} ypos={y:.4} \
             shaded-background=true draw-shadow=false color={color}"
        ),
        crate::TcLoopSource::External => format!(
            "textoverlay name=tc_text text=\"--:--:--\" font-desc=\"{font_desc}\" \
             halignment=absolute valignment=absolute xpos={x:.4} ypos={y:.4} \
             shaded-background=true draw-shadow=false color={color}"
        ),
    }
}

/// Map UI / BMD format codes to GST sink mode names.
pub fn tc_sink_mode(format_code: &str) -> String {
    match format_code.trim() {
        "auto" | "" => "1080p50".into(),
        "Hp50" | "hp50" | "1080p50" => "1080p50".into(),
        "Hi50" | "hi50" | "1080i50" => "1080i50".into(),
        "Hp25" | "hp25" | "1080p25" => "1080p25".into(),
        "Hp59.94" | "Hp5994" | "1080p5994" => "1080p5994".into(),
        "Hi59.94" | "Hi5994" | "1080i5994" => "1080i5994".into(),
        other => other.to_string(),
    }
}

fn tc_sink_geometry(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "1080i50" | "1080p25" => ("1080", "25/1"),
        "1080i5994" => ("1080", "30000/1001"),
        "1080p5994" => ("1080", "60000/1001"),
        _ => ("1080", "50/1"),
    }
}

/// Build TC burn-in: DeckLink IN → overlay → OUT + JPEG + NVENC proxy (SRT/WebRTC).
///
/// **OUT** keeps IN geometry/interlace (1080i → 1080i DeckLink).
/// **Proxy encode** (SRT + WebRTC tee `e`) is always progressive — deinterlace
/// before NVENC when the source is interlaced.
pub fn build_tc_loop_launch(opts: &TcLoopLaunchOpts) -> String {
    let src = decklink_src(&opts.input_device, &opts.input_mode);
    let overlay = tc_overlay_element(opts);
    // Prefer explicit output_mode, but default to the same as input (not playout progressive).
    let out_mode = {
        let o = opts.output_mode.trim();
        let i = opts.input_mode.trim();
        if !o.is_empty() && !o.eq_ignore_ascii_case("auto") {
            o
        } else if !i.is_empty() && !i.eq_ignore_ascii_case("auto") {
            i
        } else {
            "1080p50"
        }
    };
    let interlaced = gst_mode_is_interlaced(out_mode);
    let (height, fr) = tc_sink_geometry(out_mode);
    let vsink = decklink_video_sink(&opts.output_device, out_mode);
    let asink = decklink_audio_sink(&opts.output_device);
    let in_num = decklink_device_number(&opts.input_device);

    let live_codec = live_nvenc_codec(&opts.preset);
    let bitrate_kbit = live_nvenc_bitrate_kbit(&opts.preset);
    let gop = live_nvenc_gop(&opts.preset);
    let nv_preset = live_nvenc_preset(&opts.preset);
    let family = encode_family(live_codec);
    let parse = family.parse_element();
    let bs = family.byte_stream_caps();
    // Encode path is progressive (deinterlace when needed).
    let enc = nvenc_chain_ex(live_codec, nv_preset, bitrate_kbit, gop, false);

    let aac_bps = parse_bitrate(&opts.preset.audio_bitrate)
        .unwrap_or(192_000)
        .clamp(64_000, 320_000);
    let pairs = aac_stereo_pairs(opts.preset.audio_channels);
    let aac = mpegts_program_aac(
        "tc",
        aac_bps,
        pairs,
        &[AacMuxOut {
            mux_name: "tsmux",
            valve_prefix: None,
        }],
        false,
    );

    let srt_appsink = "queue name=q_srt max-size-buffers=8 max-size-time=0 max-size-bytes=0 \
         leaky=downstream ! \
         appsink name=srt_in emit-signals=true sync=false async=false \
         max-buffers=8 drop=true";

    // OUT: same geometry/interlace as IN. Avoid videorate on interlaced (destroys fields).
    let out_branch = if interlaced {
        format!(
            "v. ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
               videoconvert ! \
               video/x-raw,format=UYVY,width=1920,height={height},framerate={fr},\
               interlace-mode=interleaved ! \
               {vsink}"
        )
    } else {
        format!(
            "v. ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
               videoscale ! videorate skip-to-first=true ! \
               video/x-raw,format=UYVY,width=1920,height={height},framerate={fr} ! \
               {vsink}"
        )
    };

    // Proxy encode (+ WebRTC/SRT): deinterlace so NVENC/browsers get progressive.
    let enc_prefix = if interlaced {
        "v. ! queue ! deinterlace mode=auto ! "
    } else {
        "v. ! queue ! "
    };

    let thumb = if let Some(dir) = &opts.hls_dir {
        let _ = std::fs::create_dir_all(dir);
        let loc = format!("{dir}/thumb%05d.jpg");
        // Thumb only: deinterlace so JPEG is progressive 1 fps.
        format!(
            "v. ! queue max-size-buffers=2 leaky=downstream ! \
               deinterlace mode=auto ! videoconvert ! videoscale ! videorate skip-to-first=true ! \
               video/x-raw,width=640,height=360,framerate=1/1 ! \
               jpegenc quality=80 idct-method=float ! \
               multifilesink location=\"{loc}\" max-files=1 next-file=buffer \
               post-messages=false sync=false async=false"
        )
    } else {
        String::new()
    };

    format!(
        "{src} ! \
           videoconvert ! {overlay} ! tee name=v \
         {out_branch} \
         {enc_prefix}{enc} ! tee name=e \
         e. ! queue ! {parse} config-interval=-1 ! {bs} ! \
           mpegtsmux name=tsmux alignment=7 ! {srt_appsink} \
         e. ! queue leaky=downstream ! fakesink sync=false \
         decklinkaudiosrc device-number={in_num} channels=8 ! \
           audioconvert ! audio/x-raw,channels=8,rate=48000,layout=interleaved ! tee name=a \
         a. ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
           audioconvert ! audio/x-raw,format=S16LE,channels=8,rate=48000,layout=interleaved ! \
           {asink} \
         {aac} \
         {meter} \
         {thumb}",
        meter = meter_branch(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use roc_config::EncodePreset;

    #[test]
    fn parses_display_name() {
        assert_eq!(decklink_device_number("DeckLink IP 100G (1)"), 0);
        assert_eq!(decklink_device_number("DeckLink IP 100G (8)"), 7);
        assert_eq!(decklink_device_number("3"), 2);
        assert_eq!(decklink_device_number("0"), 0);
    }

    #[test]
    fn stereo_pair_matrix_selects_pair() {
        let m0 = stereo_pair_matrix(0);
        assert!(m0.contains("1.0, 0.0, 0.0"));
        let m1 = stereo_pair_matrix(1);
        assert!(m1.contains("0.0, 0.0, 1.0, 0.0"));
    }

    #[test]
    fn stereo_into_8ch_places_pair() {
        let m0 = stereo_into_8ch_matrix(0);
        assert!(m0.starts_with("<<1.0, 0.0>, <0.0, 1.0>, <0.0, 0.0>"));
        let m2 = stereo_into_8ch_matrix(2);
        assert!(m2.contains("<0.0, 0.0>, <0.0, 0.0>, <1.0, 0.0>, <0.0, 1.0>"));
    }

    #[test]
    fn encode_once_srt_udp_use_four_aac_when_8ch() {
        let mut preset = EncodePreset {
            label: "HQ".into(),
            video_codec: "nvh264enc".into(),
            video_bitrate: "12M".into(),
            video_maxrate: None,
            video_bufsize: None,
            video_preset: "low-latency-hq".into(),
            video_gop: 50,
            audio_bitrate: "192k".into(),
            audio_channels: 8,
        };
        preset.normalize_for_gst();
        let launch = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: "DeckLink IP 100G (1)".into(),
            mode: "auto".into(),
            preset,
            preview_path: None,
            record_path: None,
            srt_url: Some("srt://0.0.0.0:9101?mode=listener".into()),
            udp_egress: Some("udp://239.255.28.1:21001".into()),
            with_tee_preview: false,
        });
        assert_eq!(aac_stereo_pairs(8), 4);
        // Hydra model: one tsmux + tee; 4×AAC program only (no second SRT remux).
        assert_eq!(launch.matches("voaacenc").count(), 4);
        assert!(launch.contains("tsmux"));
        assert!(launch.contains("ts_out"));
        assert!(launch.contains("srt_in"));
        assert!(launch.contains("appsink"));
        assert!(!launch.contains("srtmux"));
        assert!(!launch.contains("srt_v_valve"));
        assert_eq!(launch.matches("tsmux.").count(), 4);
        assert!(launch.contains("voaacenc"));

        let with_thumb = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: "DeckLink IP 100G (1)".into(),
            mode: "auto".into(),
            preset: {
                let mut p = EncodePreset {
                    label: "HQ".into(),
                    video_codec: "nvh264enc".into(),
                    video_bitrate: "12M".into(),
                    video_maxrate: None,
                    video_bufsize: None,
                    video_preset: "low-latency-hq".into(),
                    video_gop: 50,
                    audio_bitrate: "192k".into(),
                    audio_channels: 8,
                };
                p.normalize_for_gst();
                p
            },
            preview_path: Some("/tmp/roc-preview/preview.m3u8".into()),
            record_path: None,
            srt_url: Some("srt://0.0.0.0:9101?mode=listener".into()),
            udp_egress: Some("udp://239.255.28.1:21001".into()),
            with_tee_preview: true,
        });
        // Program AAC only (no always-on listen HLS); JPEG thumb for grid.
        assert!(with_thumb.matches("voaacenc").count() >= 4);
        assert!(with_thumb.contains("jpegenc"));
        assert!(with_thumb.contains("thumb%05d.jpg"));
        assert!(!with_thumb.contains("hls_l0"));
        assert!(!with_thumb.contains("hlssink2"));
        assert!(with_thumb.contains("appsink name=srt_in"));
        assert!(with_thumb.contains("interval=33000000"));
        // UDP-only graphs still expose srt_in so SRT can hot-attach without relaunch.
        let udp_only = build_capture_encode_once_launch(&CaptureLaunchOpts {
            device: "DeckLink IP 100G (1)".into(),
            mode: "auto".into(),
            preset: {
                let mut p = EncodePreset {
                    label: "HQ".into(),
                    video_codec: "nvh264enc".into(),
                    video_bitrate: "12M".into(),
                    video_maxrate: None,
                    video_bufsize: None,
                    video_preset: "low-latency-hq".into(),
                    video_gop: 50,
                    audio_bitrate: "192k".into(),
                    audio_channels: 2,
                };
                p.normalize_for_gst();
                p
            },
            preview_path: None,
            record_path: None,
            srt_url: None,
            udp_egress: Some("udp://239.255.28.1:21001".into()),
            with_tee_preview: false,
        });
        assert!(udp_only.contains("appsink name=srt_in"), "{udp_only}");
        assert!(udp_only.contains("tee name=ts_out"), "{udp_only}");
        assert!(
            udp_only.contains("tee name=raw")
                && udp_only.find("tee name=raw").unwrap()
                    < udp_only.find("deinterlace").unwrap(),
            "mezz tee raw must precede deinterlace: {udp_only}"
        );
    }

    #[test]
    fn nvenc_chain_is_player_friendly() {
        let h264 = nvenc_chain("nvh264enc", "hq", 4000, 50);
        assert!(h264.contains("repeat-sequence-header=true"));
        assert!(h264.contains("zerolatency=true"));
        assert!(h264.contains("bframes=0"));
        assert!(h264.contains("stream-format=byte-stream"));
        assert!(h264.contains("h264parse config-interval=-1"));
        let hevc = nvenc_chain("nvh265enc", "hq", 8000, 50);
        assert!(hevc.contains("repeat-sequence-header=true"));
        assert!(hevc.contains("zerolatency=true"));
        assert!(hevc.contains("stream-format=byte-stream"));
        assert!(!hevc.contains("bframes="));
        assert!(hevc.contains("h265parse config-interval=-1"));
    }

    #[test]
    fn gst_mode_looks_interlaced_detects_scan() {
        assert!(gst_mode_looks_interlaced("1080i50"));
        assert!(gst_mode_looks_interlaced("1920x1080i50"));
        assert!(!gst_mode_looks_interlaced("1080p50"));
        assert!(!gst_mode_looks_interlaced("Hp50"));
        assert!(!gst_mode_looks_interlaced(""));
    }

    #[test]
    fn tc_loop_preserves_interlaced_in_to_out() {
        let mut preset = EncodePreset {
            label: "Proxy".into(),
            video_codec: "nvh264enc".into(),
            video_bitrate: "8M".into(),
            video_maxrate: None,
            video_bufsize: None,
            video_preset: "low-latency-hq".into(),
            video_gop: 50,
            audio_bitrate: "192k".into(),
            audio_channels: 8,
        };
        preset.normalize_for_gst();
        let launch = build_tc_loop_launch(&TcLoopLaunchOpts {
            input_device: "DeckLink IP 100G (1)".into(),
            output_device: "DeckLink IP 100G (2)".into(),
            input_mode: "1080i50".into(),
            output_mode: "1080i50".into(),
            source: crate::TcLoopSource::Tod,
            udp_port: 9301,
            fontsize: 48,
            opacity: 0.9,
            position: crate::TcLoopPosition::TopLeft,
            x: 0.04,
            y: 0.04,
            preset,
            hls_dir: Some("/tmp/roc-tc".into()),
        });
        assert!(launch.contains("mode=1080i50"), "{launch}");
        assert!(launch.contains("interlace-mode=interleaved"), "{launch}");
        assert!(!launch.contains("interlaced-encoding="), "{launch}");
        assert!(launch.contains("appsink name=srt_in"), "{launch}");
        assert!(launch.contains("tee name=e"), "{launch}");
        // OUT stays interlaced; encode + JPEG deinterlace for progressive proxy/WebRTC/SRT.
        assert_eq!(
            launch.matches("deinterlace").count(),
            2,
            "expected encode+thumb deinterlace: {launch}"
        );
        // Encode branch deinterlaces before NVENC.
        assert!(
            launch.contains("deinterlace mode=auto ! ")
                && launch.contains("nvh264enc"),
            "{launch}"
        );
    }
}
