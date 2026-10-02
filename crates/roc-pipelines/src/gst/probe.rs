use anyhow::Result;
use roc_devices::{DeviceDirection, DeviceInfo, DeviceProbeReport};

pub fn probe_gst_devices() -> Result<DeviceProbeReport> {
    let mut devices = Vec::new();
    let mut encoders = Vec::new();
    let mut notes = Vec::new();

    for name in [
        "decklinkvideosrc",
        "decklinkvideosink",
        "decklinkaudiosrc",
        "decklinkaudiosink",
    ] {
        if gstreamer::ElementFactory::find(name).is_some() {
            notes.push(format!("plugin element present: {name}"));
        } else {
            notes.push(format!("MISSING element: {name}"));
        }
    }

    for name in [
        "nvh264enc",
        "nvd3d11h264enc",
        "nvautogpuh264enc",
        "x264enc",
        "srtsink",
        "srtsrc",
        "mpegtsmux",
        "mp4mux",
    ] {
        if gstreamer::ElementFactory::find(name).is_some() {
            encoders.push(name.to_string());
        } else {
            notes.push(format!("element not found: {name}"));
        }
    }

    if gstreamer::ElementFactory::find("decklinkvideosrc").is_some() {
        for idx in 0..8u32 {
            let name = format!("DeckLink IP 100G ({})", idx + 1);
            devices.push(DeviceInfo {
                name: name.clone(),
                direction: DeviceDirection::Input,
                unique_id: Some(format!("idx-{idx}")),
            });
            devices.push(DeviceInfo {
                name,
                direction: DeviceDirection::Output,
                unique_id: Some(format!("idx-out-{idx}")),
            });
        }
        notes.push(
            "Verify exact device-name with: gst-device-monitor-1.0 Video/Source".into(),
        );
    } else {
        notes.push(
            "decklinkvideosrc missing — install gst-plugins-bad built with Blackmagic DeckLink SDK"
                .into(),
        );
    }

    Ok(DeviceProbeReport {
        backend: "gstreamer".into(),
        devices,
        encoders,
        notes,
    })
}
