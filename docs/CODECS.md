# Codecs roadmap

## Supported now (NVENC live + REC)

| Preset | Element | Notes |
|--------|---------|--------|
| `proxy` / `hq` / `mezz` | `nvh264enc` | Proven on capture host (1080i50 → NV12 → cudaupload) |
| `hq_hevc` / `mezz_hevc` | `nvh265enc` | Same upload path; spike: `--codec hevc` |

Live/SRT always uses NVENC (encode-once tee `e`).

## Mezz REC (raw tee + NTP timecode)

Capture graph:

`decklink → tee raw → (mezz) | deinterlace → tee t → NVENC / JPEG`

| Preset | Codec | Container | Notes |
|--------|-------|-----------|--------|
| `dnxhd_sq` / `dnxhd_hq` / `dnxhd_hqx` | `avenc_dnxhd` | `.mxf` (`mxfmux`) | Class selects SQ/HQ/HQX; **bitrate + scan follow live signal** (e.g. 1080i50 HQ → 185 Mbps interlaced). Tap is **pre-deinterlace** `raw`. HQX needs a real 10-bit source (`v210`). |
| `dnxhd_145` / `dnxhd_185` | same | `.mxf` | Legacy ids → SQ / HQ |
| `xavc_intra_hd` | `x264enc` High 4:2:2 Intra | `.mxf` (`mxfmux`) | Open-source XAVC Intra HD **approximation** (not Sony Class 100) |

### DNxHD operating points (1080)

| Signal | SQ (8-bit) | HQ (8-bit) | HQX (10-bit) |
|--------|------------|------------|--------------|
| 1080i50 / 1080p25 | 120 | 185 | 185x |
| 1080i59.94 / 1080p29.97 | 145 | 220 | 220x |
| 1080p50 | 240 | 365 | 365x |

Timecode: GStreamer `timecodestamper source=rtc set=always` — uses the host
real-time clock. Capture host has **no PTP**; chrony syncs RTC from LAN NTP:

- `10.199.6.10` (`one.time.nepsweden.local`)
- `10.199.6.14` (`two.time.nepsweden.local`)

Install/refresh: [`deploy/remote-ntp-roc.sh`](../deploy/remote-ntp-roc.sh) +
[`deploy/chrony-roc-ntp.sources`](../deploy/chrony-roc-ntp.sources).

When `record_preset` is a mezz codec (`dnxhd_*` / `xavc_*`), SRT/UDP/preview
keep the channel's `encode_preset` (NVENC proxy); only the REC branch uses the
mezz encoder from tee `raw`. Proxy and REC are selected independently in the
UI (`encode_preset` vs `record_preset`).

## Later

| Codec | Status | Plan |
|-------|--------|------|
| ProRes | `avenc_prores` present | Optional Apple mezz |
| True Sony XAVC | needs vendor tooling | Replace x264 Intra approx if required |
| AV1 NVENC | P2000 unlikely | Skip |
| **TC interlaced SRT/WebRTC** | done via deinterlace | DeckLink OUT stays interlaced; proxy encode deinterlaces before NVENC so SRT + WebRTC are progressive |
| DNxHR / 4:4:4 | not started | UHD / 444 mezz |

## Deferred audio

Proxy/SRT 8ch is four AAC stereo pairs in one MPEG-TS program (not one 8ch AAC
stream). Players that only open the first audio PID will still look like 2ch.
Mezz MXF uses PCM (stereo or 8ch as four stereo pairs) for both DNxHD and XAVC.
