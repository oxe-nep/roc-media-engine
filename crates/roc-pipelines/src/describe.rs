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
    // Always lock to a concrete mode for the live graph (caller probes when config is `auto`).
    // drop-no-signal-frames keeps the pipeline alive across brief ST 2110 gaps.
    let mode = if mode.trim().is_empty() || mode.eq_ignore_ascii_case("auto") {
        "1080p50" // last-resort fallback; prefer probe_input_format first
    } else {
        mode.trim()
    };
    format!(
        "decklinkvideosrc name=dlsrc device-number={} mode={} drop-no-signal-frames=true",
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

/// 8→2 matrix selecting stereo pair `pair` (0=ch1-2 … 3=ch7-8).
fn stereo_pair_matrix(pair: usize) -> String {
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

/// Four audio-only listen HLS playlists (`listen_0.m3u8` … `listen_3.m3u8`).
fn listen_hls_branches(hls_dir: &str) -> String {
    let mut parts = Vec::with_capacity(4);
    for pair in 0..4 {
        let matrix = stereo_pair_matrix(pair);
        let playlist = format!("{hls_dir}/listen_{pair}.m3u8");
        let seg = format!("{hls_dir}/l{pair}_%05d.ts");
        parts.push(format!(
            "a. ! queue max-size-buffers=64 leaky=downstream ! \
             audiomixmatrix in-channels=8 out-channels=2 mode=manual matrix=\"{matrix}\" ! \
             audioconvert ! voaacenc bitrate=128000 ! aacparse ! mpegtsmux alignment=7 ! \
             hlssink location=\"{seg}\" playlist-location=\"{playlist}\" \
             target-duration=1 max-files=6 playlist-length=6"
        ));
    }
    parts.join(" ")
}

fn hls_dir_from_playlist(playlist: &str) -> String {
    std::path::Path::new(playlist)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

fn encode_family(video_codec: &str) -> EncodeFamily {
    let c = video_codec.to_ascii_lowercase();
    if c.contains("265") || c.contains("hevc") {
        EncodeFamily::Hevc
    } else {
        EncodeFamily::H264
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
}

/// NVENC encode chain (H.264 or H.265) via NV12 + cudaupload.
fn nvenc_chain(video_codec: &str, preset: &str, bitrate_kbit: u64, gop: u32) -> String {
    let family = encode_family(video_codec);
    let enc = family.encoder_element();
    let parse = family.parse_element();
    format!(
        "videoconvert ! video/x-raw,format=NV12 ! cudaupload ! \
         {enc} preset={preset} bitrate={bitrate_kbit} gop-size={gop} ! \
         {parse} config-interval=-1"
    )
}

fn decklink_video_sink(device: &str, mode: &str) -> String {
    format!(
        "decklinkvideosink device-number={} mode={}",
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
            format!("{dir}/seg%05d.ts")
        };
        branches.push(format!(
            "t. ! queue max-size-buffers=3 leaky=downstream ! videorate ! video/x-raw,framerate=10/1 ! \
             videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 key-int-max=10 bframes=0 ! \
             video/x-h264,profile=baseline ! h264parse ! \
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

/// Optimized capture: encode once, then tee the bitstream to REC / SRT / UDP.
pub fn build_capture_encode_once_launch(opts: &CaptureLaunchOpts) -> String {
    let bitrate_kbit = parse_bitrate(&opts.preset.video_bitrate)
        .map(|b| b / 1000)
        .unwrap_or(12_000);
    let gop = opts.preset.video_gop;
    let preset = &opts.preset.video_preset;
    let family = encode_family(&opts.preset.video_codec);
    let parse = family.parse_element();
    let src = decklink_src(&opts.device, &opts.mode);

    let mut out_branches = Vec::new();
    if let Some(path) = &opts.record_path {
        out_branches.push(format!(
            "e. ! queue ! {parse} ! mp4mux fragment-duration=1000 ! filesink location=\"{path}\" sync=false"
        ));
    }
    if let Some(url) = &opts.srt_url {
        out_branches.push(format!(
            "e. ! queue ! {parse} ! mpegtsmux alignment=7 ! srtsink uri=\"{url}\" wait-for-connection=false"
        ));
    }
    if let Some(url) = &opts.udp_egress {
        let (host, port) = udp_host_port(url);
        out_branches.push(format!(
            "e. ! queue ! {parse} ! mpegtsmux alignment=7 ! udpsink host={host} port={port} sync=false async=false"
        ));
    }
    out_branches.push("e. ! queue leaky=downstream ! fakesink sync=false".into());

    let preview = if opts.with_tee_preview || opts.preview_path.is_some() {
        let playlist = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview/preview.m3u8".into());
        let dir = hls_dir_from_playlist(&playlist);
        let seg = format!("{dir}/seg%05d.ts");
        format!(
            "t. ! queue max-size-buffers=3 leaky=downstream ! videorate ! video/x-raw,framerate=10/1 ! \
             videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 key-int-max=10 bframes=0 ! \
             video/x-h264,profile=baseline ! h264parse ! \
             hlssink2 location=\"{seg}\" playlist-location=\"{playlist}\" \
             target-duration=1 max-files=6 playlist-length=6"
        )
    } else {
        String::new()
    };

    let (audio_src, listen) = if opts.with_tee_preview || opts.preview_path.is_some() {
        let playlist = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview/preview.m3u8".into());
        let dir = hls_dir_from_playlist(&playlist);
        let num = decklink_device_number(&opts.device);
        (
            format!(
                "decklinkaudiosrc device-number={num} channels=8 ! \
                 audioconvert ! audio/x-raw,channels=8,rate=48000,layout=interleaved ! tee name=a"
            ),
            listen_hls_branches(&dir),
        )
    } else {
        (String::new(), String::new())
    };

    format!(
        "{src} ! \
         deinterlace mode=auto ! tee name=t \
         {audio_src} \
         t. ! queue ! {enc} ! \
         tee name=e \
         {out_branches} \
         {preview} \
         {listen}",
        enc = nvenc_chain(&opts.preset.video_codec, preset, bitrate_kbit, gop),
        out_branches = out_branches.join(" "),
    )
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
    let mode = match opts.format_code.as_str() {
        "Hp50" | "hp50" => "1080p50",
        "Hi50" | "hi50" => "1080i50",
        "Hp25" | "hp25" => "1080p25",
        "Hp59.94" | "Hp5994" => "1080p5994",
        "Hi59.94" | "Hi5994" => "1080i5994",
        other => other,
    };

    format!(
        "{src} \
         d. ! queue ! h264parse ! avdec_h264 ! videoconvert ! \
         video/x-raw,format=v210 ! \
         {vsink} \
         d. ! queue ! aacparse ! avdec_aac ! audioconvert ! audioresample ! \
         audio/x-raw,format=S16LE,channels=2 ! \
         {asink}",
        vsink = decklink_video_sink(&opts.device, mode),
        asink = decklink_audio_sink(&opts.device),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
