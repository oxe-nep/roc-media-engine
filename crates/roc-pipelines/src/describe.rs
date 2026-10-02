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

/// Spike: DeckLink → deinterlace → NVENC → optional tee → file (+ preview).
pub fn build_spike_tee_launch(device: &str, output_mp4: &str, with_preview: bool) -> String {
    let enc = "nvh264enc preset=low-latency-hq bitrate=12000 gop-size=50 ! h264parse config-interval=-1";
    if with_preview {
        format!(
            "decklinkvideosrc device-name=\"{device}\" ! \
             deinterlace ! videoconvert ! video/x-raw,format=NV12 ! \
             tee name=t \
             t. ! queue ! {enc} ! mp4mux fragment-duration=1000 ! filesink location=\"{output_mp4}\" \
             t. ! queue ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 ! \
             h264parse ! mpegtsmux ! filesink location=\"{output_mp4}.preview.ts\""
        )
    } else {
        format!(
            "decklinkvideosrc device-name=\"{device}\" ! \
             deinterlace ! videoconvert ! video/x-raw,format=NV12 ! \
             {enc} ! mp4mux fragment-duration=1000 ! filesink location=\"{output_mp4}\""
        )
    }
}

pub fn build_capture_launch(opts: &CaptureLaunchOpts) -> String {
    let bitrate_kbit = parse_bitrate(&opts.preset.video_bitrate)
        .map(|b| b / 1000)
        .unwrap_or(12_000);
    let gop = opts.preset.video_gop;
    let preset = &opts.preset.video_preset;
    let enc = format!(
        "nvh264enc preset={preset} bitrate={bitrate_kbit} gop-size={gop} ! h264parse config-interval=-1"
    );

    let mut branches = Vec::new();

    if let Some(path) = &opts.record_path {
        branches.push(format!(
            "t. ! queue name=q_rec ! {enc} ! mp4mux fragment-duration=1000 ! filesink location=\"{path}\" sync=false"
        ));
    }

    if let Some(url) = &opts.srt_url {
        // Re-use same encoded pattern: separate encode branch for SRT if no shared encoded tee yet.
        // Production path: encode once, tee Annex-B into mpegtsmux → srtsink.
        branches.push(format!(
            "t. ! queue name=q_srt ! {enc} ! mpegtsmux alignment=7 ! srtsink uri=\"{url}\" wait-for-connection=false"
        ));
    }

    if let Some(udp) = &opts.udp_egress {
        let _ = udp;
        branches.push(
            "t. ! queue name=q_udp ! {enc} ! mpegtsmux alignment=7 ! udpsink host=239.255.28.1 port=21001 sync=false async=false"
                .replace("{enc}", &enc),
        );
    }

    if opts.with_tee_preview || opts.preview_path.is_some() {
        let prev = opts
            .preview_path
            .clone()
            .unwrap_or_else(|| "/tmp/roc-preview.ts".into());
        branches.push(format!(
            "t. ! queue name=q_prev ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 key-int-max=50 ! \
             h264parse ! mpegtsmux ! filesink location=\"{prev}\" sync=false"
        ));
    }

    // Always keep a fakesink branch so tee has at least one consumer when idle.
    branches.push("t. ! queue name=q_idle leaky=downstream ! fakesink sync=false".into());

    format!(
        "decklinkvideosrc device-name=\"{device}\" ! \
         deinterlace mode=auto ! videoconvert ! video/x-raw,format=NV12 ! \
         tee name=t \
         {branches}",
        device = opts.device,
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
        // Caller should set host/port from config; placeholder uses channel-style defaults.
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
            "t. ! queue ! videoscale ! video/x-raw,width=640,height=360 ! \
             x264enc tune=zerolatency speed-preset=ultrafast bitrate=800 ! \
             h264parse ! mpegtsmux ! filesink location=\"{prev}\" sync=false"
        )
    } else {
        String::new()
    };

    format!(
        "decklinkvideosrc device-name=\"{device}\" ! \
         deinterlace mode=auto ! videoconvert ! video/x-raw,format=NV12 ! \
         tee name=t \
         t. ! queue ! nvh264enc preset={preset} bitrate={bitrate_kbit} gop-size={gop} ! \
         tee name=e \
         {out_branches} \
         {preview}",
        device = opts.device,
        out_branches = out_branches.join(" "),
    )
}

pub fn build_playout_launch(opts: &PlayoutLaunchOpts) -> String {
    let src = if opts.source.starts_with("srt://") {
        format!("srtsrc uri=\"{}\" ! tsdemux name=d", opts.source)
    } else if opts.source.ends_with(".ts") {
        format!(
            "filesrc location=\"{}\" ! tsdemux name=d",
            opts.source
        )
    } else {
        format!(
            "filesrc location=\"{}\" ! qtdemux name=d",
            opts.source
        )
    };

    format!(
        "{src} \
         d. ! queue ! h264parse ! avdec_h264 ! videoconvert ! \
         video/x-raw,format=v210 ! \
         decklinkvideosink device-name=\"{device}\" mode={mode} \
         d. ! queue ! aacparse ! avdec_aac ! audioconvert ! audioresample ! \
         audio/x-raw,format=S16LE,channels=2 ! \
         decklinkaudiosink device-name=\"{device}\"",
        device = opts.device,
        mode = opts.format_code,
    )
}
