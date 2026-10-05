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

When a mezz preset is selected, SRT/UDP/preview keep a fixed H.264 NVENC proxy;
only the REC branch switches to DNxHD/XAVC.

## Later

| Codec | Status | Plan |
|-------|--------|------|
| ProRes | `avenc_prores` present | Optional Apple mezz |
| True Sony XAVC | needs vendor tooling | Replace x264 Intra approx if required |
| AV1 NVENC | P2000 unlikely | Skip |

## Deferred audio

Full 8ch AAC (4× pairs) on SRT/REC remains deferred; stereo default stays.
Mezz MXF uses stereo PCM for both DNxHD and XAVC.
