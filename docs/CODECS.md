# Codecs roadmap

## Supported now (NVENC live + REC)

| Preset | Element | Notes |
|--------|---------|--------|
| `proxy` / `hq` / `mezz` | `nvh264enc` | Proven on capture host (1080i50 → NV12 → cudaupload) |
| `hq_hevc` / `mezz_hevc` | `nvh265enc` | Same upload path; spike: `--codec hevc` |

Live/SRT always uses NVENC (encode-once tee `e`).

## Mezz REC (raw tee + NTP timecode)

| Preset | Codec | Container | Notes |
|--------|-------|-----------|--------|
| `dnxhd_145` / `dnxhd_185` | `avenc_dnxhd` | `.mxf` (`mxfmux`) | Y42B from raw tee `t` (qtmux cannot take DNxHD on this host) |
| `xavc_intra_hd` | `x264enc` High 4:2:2 Intra | `.mxf` (`mxfmux`) | Open-source XAVC Intra HD approximation |

Timecode: GStreamer `timecodestamper source=rtc set=always` — uses the host
real-time clock. Capture host has **no PTP**; chrony syncs RTC from LAN NTP:

- `10.199.6.10` (`one.time.nepsweden.local`)
- `10.199.6.14` (`two.time.nepsweden.local`)

Install/refresh: [`deploy/remote-ntp-roc.sh`](../deploy/remote-ntp-roc.sh) +
[`deploy/chrony-roc-ntp.sources`](../deploy/chrony-roc-ntp.sources).

When `record_preset` is a mezz codec (`dnxhd_*` / `xavc_*`), SRT/UDP/preview
keep the channel's `encode_preset` (NVENC proxy); only the REC branch uses the
mezz encoder from raw tee `t`. Proxy and REC are selected independently in the
UI (`encode_preset` vs `record_preset`).

## Later

| Codec | Status | Plan |
|-------|--------|------|
| ProRes | `avenc_prores` present | Optional Apple mezz |
| True Sony XAVC | needs vendor tooling | Replace x264 Intra approx if required |
| AV1 NVENC | P2000 unlikely | Skip |

## Deferred audio

Proxy/SRT 8ch is four AAC stereo pairs in one MPEG-TS program (not one 8ch AAC
stream). Players that only open the first audio PID will still look like 2ch.
Mezz MXF uses PCM (stereo or 8ch as four stereo pairs) for both DNxHD and XAVC.
