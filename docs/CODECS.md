# Codecs roadmap

## Supported now (NVENC)

| Preset | Element | Notes |
|--------|---------|--------|
| `proxy` / `hq` / `mezz` | `nvh264enc` | Proven on capture host (1080i50 → NV12 → cudaupload) |
| `hq_hevc` / `mezz_hevc` | `nvh265enc` | Same upload path; spike: `--codec hevc` |

Spike:

```bash
./target/release/spike-decklink-nvenc --codec h264 --preview --duration-secs 30
./target/release/spike-decklink-nvenc --codec hevc --preview --duration-secs 30 \
  --output /tmp/roc-spike-hevc.mp4
```

## Next implementation targets

H.264/HEVC capture+REC+SRT is solid. Next mezz codecs (priority order):

| Codec | Host status | Plan |
|-------|-------------|------|
| **DNxHD / VC-3** | `avenc_dnxhd` / `avdec_dnxhd` present via libav | Dedicated mezz preset (fixed bitrate profiles), MOV/MXF mux on REC branch |
| **XAVC** | not wired yet | Probe host encoders (`avenc_*` / Sony XAVC profiles); mezz preset + compatible mux (MXF preferred) |
| ProRes | not probed | Later if needed for Apple mezz |
| AV1 NVENC | depends on GPU (P2000: unlikely) | Skip unless hardware supports |

### Suggested land order

1. Probe capture host: `gst-inspect-1.0 avenc_dnxhd`, list any XAVC-capable elements / ffmpeg wrappers
2. Add encode presets (DNxHD profiles first) without breaking NVENC tee path — mezz may be a second encode branch or alternate REC branch
3. Wire REC mux (MOV for DNxHD, MXF for XAVC/DNxHD as needed)
4. UI preset picker already uses `/api/encode/presets` — register new ids there

## Deferred audio

Full 8ch AAC (4× pairs) on SRT/REC remains deferred until mezz codec path is chosen; stereo default stays.
