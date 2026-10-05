use roc_config::Config;

use crate::{mock::MockBackend, PipelineBackend};

#[test]
fn nvenc_limit_blocks_ninth_session() {
    let cfg = Config::example();
    let backend = MockBackend::new(8);
    for ch in &cfg.channels {
        let encode = cfg.preset_for_channel(ch).unwrap();
        let record = cfg.record_preset_for_channel(ch).unwrap();
        backend.ensure_channel(ch, encode, record).unwrap();
        backend.start_capture(ch.id).unwrap();
    }
    assert_eq!(backend.nvenc_used(), 8);
    let mut extra = cfg.channels[0].clone();
    extra.id = 99;
    extra.name = "overflow".into();
    let encode = cfg.preset_for_channel(&extra).unwrap();
    let record = cfg.record_preset_for_channel(&extra).unwrap();
    backend.ensure_channel(&extra, encode, record).unwrap();
    assert!(backend.start_capture(99).is_err());
}

#[test]
fn srt_and_record_require_capture() {
    let cfg = Config::example();
    let backend = MockBackend::new(8);
    let ch = &cfg.channels[0];
    let encode = cfg.preset_for_channel(ch).unwrap();
    let record = cfg.record_preset_for_channel(ch).unwrap();
    backend.ensure_channel(ch, encode, record).unwrap();
    assert!(backend.start_recording(ch.id, "/tmp/x.mp4").is_err());
    backend.start_capture(ch.id).unwrap();
    backend.start_recording(ch.id, "/tmp/x.mp4").unwrap();
    backend
        .start_srt(ch.id, "srt://0.0.0.0:9101?mode=listener")
        .unwrap();
    let snap = backend.channel_snapshot(ch.id).unwrap();
    assert!(snap.recording);
    assert!(snap.srt);
}

#[test]
fn proxy_and_hq_recording_are_independent() {
    let cfg = Config::example();
    let backend = MockBackend::new(8);
    let ch = &cfg.channels[0];
    let encode = cfg.preset_for_channel(ch).unwrap();
    let record = cfg.record_preset_for_channel(ch).unwrap();
    backend.ensure_channel(ch, encode, record).unwrap();
    backend.start_capture(ch.id).unwrap();

    backend.start_proxy_recording(ch.id, "/tmp/p.mp4").unwrap();
    let snap = backend.channel_snapshot(ch.id).unwrap();
    assert!(snap.recording && snap.proxy_recording && !snap.hq_recording);
    assert_eq!(snap.recording_path.as_deref(), Some("/tmp/p.mp4"));

    // Starting HQ alongside proxy; legacy path prefers HQ.
    backend.start_hq_recording(ch.id, "/tmp/h.mxf").unwrap();
    let snap = backend.channel_snapshot(ch.id).unwrap();
    assert!(snap.proxy_recording && snap.hq_recording);
    assert_eq!(snap.proxy_recording_path.as_deref(), Some("/tmp/p.mp4"));
    assert_eq!(snap.hq_recording_path.as_deref(), Some("/tmp/h.mxf"));
    assert_eq!(snap.recording_path.as_deref(), Some("/tmp/h.mxf"));
    assert!(backend.start_hq_recording(ch.id, "/tmp/h2.mxf").is_err());

    // Record preset change is blocked only while HQ records.
    assert!(backend
        .apply_record_preset(ch.id, "hq", record)
        .is_err());

    // Legacy stop_recording = HQ stop; proxy keeps running.
    backend.stop_recording(ch.id).unwrap();
    let snap = backend.channel_snapshot(ch.id).unwrap();
    assert!(snap.recording && snap.proxy_recording && !snap.hq_recording);
    assert_eq!(snap.recording_path.as_deref(), Some("/tmp/p.mp4"));
    assert!(backend.apply_record_preset(ch.id, "hq", record).is_ok());

    backend.stop_proxy_recording(ch.id).unwrap();
    let snap = backend.channel_snapshot(ch.id).unwrap();
    assert!(!snap.recording);
    assert!(snap.recording_path.is_none());
}
