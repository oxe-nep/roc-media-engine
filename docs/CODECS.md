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

## Next (not yet wired)

| Codec | Host status | Plan |
|-------|-------------|------|
| **DNxHD / VC-3** | `avenc_dnxhd` / `avdec_dnxhd` present via libav | Dedicated mezz preset (fixed bitrate profiles), MOV/MXF mux — **after** HEVC is stable in REC/SRT |
| ProRes | not probed | Later if needed for Apple mezz |
| AV1 NVENC | depends on GPU (P2000: unlikely) | Skip unless hardware supports |

One thing at a time: land HEVC in encode presets + spike first; DNxHD as a separate Fas once H.264/H.265 capture+REC is solid.
