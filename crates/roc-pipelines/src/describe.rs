//! Human-readable / gst-launch-1.0 compatible pipeline strings.

use roc_config::EncodePreset;

use crate::parse_bitrate;

#[derive(Debug, Clone)]
pub struct CaptureLaunchOpts {
    pub device: String,
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

fn decklink_src(device: &str) -> String {
    // Lock mode to avoid auto-detect renegotiation (SD→HD) which breaks live graphs.
    // drop-no-signal-frames keeps the pipeline alive across brief ST 2110 gaps.
    format!(
        "decklinkvideosrc device-number={} mode=1080i50 drop-no-signal-frames=true",
        decklink_device_number(device)
    )
}

fn nvenc_chain(preset: &str, bitrate_kbit: u64, gop: u32) -> String {
    // nvh264enc on this host accepts CUDAMemory NV12 best via cudaupload.
    format!(
        "videoconvert ! video/x-raw,format=NV12 ! cudaupload ! \
         nvh264enc preset={preset} bitrate={bitrate_kbit} gop-size={gop} ! \
         h264parse config-interval=-1"
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

/// Spike: DeckLink → deinterlace → NVENC → optional tee → file (+ preview).
pub fn build_spike_tee_launch(device: &str, output_mp4: &str, with_preview: bool) -> String {
    let src = decklink_src(device);
    let enc = nvenc_chain("low-latency-hq", 12_000, 50);
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
    let enc = nvenc_chain(preset, bitrate_kbit, gop);
    let src = decklink_src(&opts.device);

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
        branches.push(format!(
            "t. ! queue name=q_udp ! {enc} ! mpegtsmux alignment=7 ! udpsink host=239.255.28.1 port=21001 sync=false async=false"
        ));
    }

    if opts.with_tee_preview || opts.preview_path.is_some() {
        let prev = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview.ts".into());
        branches.push(format!(
            "t. ! queue name=q_prev ! videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 key-int-max=50 ! \
             h264parse ! mpegtsmux ! filesink location=\"{prev}\" sync=false"
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
    let src = decklink_src(&opts.device);

    let mut out_branches = Vec::new();
    if let Some(path) = &opts.record_path {
        out_branches.push(format!(
            "e. ! queue ! h264parse ! mp4mux fragment-duration=1000 ! filesink location=\"{path}\" sync=false"
        ));
    }
    if let Some(url) = &opts.srt_url {
        out_branches.push(format!(
            "e. ! queue ! h264parse ! mpegtsmux alignment=7 ! srtsink uri=\"{url}\" wait-for-connection=false"
        ));
    }
    if opts.udp_egress.is_some() {
        out_branches.push(
            "e. ! queue ! h264parse ! mpegtsmux alignment=7 ! udpsink host=239.255.28.1 port=21001 sync=false async=false"
                .into(),
        );
    }
    out_branches.push("e. ! queue leaky=downstream ! fakesink sync=false".into());

    let preview = if opts.with_tee_preview || opts.preview_path.is_some() {
        let prev = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview.ts".into());
        format!(
            "t. ! queue ! videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 ! \
             h264parse ! mpegtsmux ! filesink location=\"{prev}\" sync=false"
        )
    } else {
        String::new()
    };

    format!(
        "{src} ! \
         deinterlace mode=auto ! tee name=t \
         t. ! queue ! {enc} ! \
         tee name=e \
         {out_branches} \
         {preview}",
        enc = nvenc_chain(preset, bitrate_kbit, gop),
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
}
