use roc_config::Config;

use crate::{mock::MockBackend, PipelineBackend};

#[test]
fn nvenc_limit_blocks_ninth_session() {
    let cfg = Config::example();
    let backend = MockBackend::new(8);
    for ch in &cfg.channels {
        let preset = cfg.preset_for_channel(ch).unwrap();
        backend.ensure_channel(ch, preset).unwrap();
        backend.start_capture(ch.id).unwrap();
    }
    assert_eq!(backend.nvenc_used(), 8);
    let mut extra = cfg.channels[0].clone();
    extra.id = 99;
    extra.name = "overflow".into();
    let preset = cfg.preset_for_channel(&extra).unwrap();
    backend.ensure_channel(&extra, preset).unwrap();
    assert!(backend.start_capture(99).is_err());
}

#[test]
fn srt_and_record_require_capture() {
    let cfg = Config::example();
    let backend = MockBackend::new(8);
    let ch = &cfg.channels[0];
    let preset = cfg.preset_for_channel(ch).unwrap();
    backend.ensure_channel(ch, preset).unwrap();
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
