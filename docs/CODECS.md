# Codecs roadmap

## Supported now (NVENC live + REC)

| Preset | Element | Notes |
|--------|---------|--------|
| `proxy` / `hq` / `mezz` | `nvh264enc` | Proven on capture host (1080i50 → NV12 → cudaupload) |
| `hq_hevc` / `mezz_hevc` | `nvh265enc` | Same upload path; spike: `--codec hevc` |

Live/SRT always uses NVENC (encode-once tee `e`).

## Mezz REC (raw / progressive tee + NTP timecode)

Capture graph:

`decklink → tee raw → (DNxHD) | deinterlace → tee t → NVENC / JPEG / ProRes`

| Preset | Codec | Container | Notes |
|--------|-------|-----------|--------|
| `dnxhd_sq` / `dnxhd_hq` / `dnxhd_hqx` | `avenc_dnxhd` | `.mxf` (`mxfmux`) | Class selects SQ/HQ/HQX; **bitrate + scan follow live signal** (e.g. 1080i50 HQ → 185 Mbps interlaced). Tap is **pre-deinterlace** `raw`. HQX needs a real 10-bit source (`v210`). 10-bit source + SQ/HQ quietly downconverts to 8-bit (`Y42B`). MediaInfo may label HQ as “220” (NTSC family name) even when the OP is **185**. |
| `dnxhd_145` / `dnxhd_185` | same | `.mxf` | Legacy ids → SQ / HQ |
| `prores_proxy` / `prores_lt` / `prores_422` / `prores_hq` | `avenc_prores_ks` | `.mov` (`qtmux`) | **Experimental** on current capture hardware (CPU encode + NFS). Profile-driven (Proxy / LT / 422 / HQ). Tap is **progressive** tee `t`. Prefer **DNxHD** for reliable mezz until the host is upgraded. |

### DNxHD operating points (1080 — **i50 / p50 only**)

| Signal | SQ (8-bit) | HQ (8-bit) | HQX (10-bit) |
|--------|------------|------------|--------------|
| 1080i50 | 120 | 185 | 185x |
| 1080p50 | 240 | 365 | 365x |

Other rates (e.g. i59.94) are rejected with a clear error.

Timecode: GStreamer `timecodestamper source=rtc set=always` — uses the host
real-time clock. Capture host has **no PTP**; chrony syncs RTC from LAN NTP:

- `10.199.6.10` (`one.time.nepsweden.local`)
- `10.199.6.14` (`two.time.nepsweden.local`)

Install/refresh: [`deploy/remote-ntp-roc.sh`](../deploy/remote-ntp-roc.sh) +
[`deploy/chrony-roc-ntp.sources`](../deploy/chrony-roc-ntp.sources).

When `record_preset` is a mezz codec (`dnxhd_*` / `prores_*`), SRT/UDP/preview
keep the channel's `encode_preset` (NVENC proxy); only the REC branch uses the
mezz encoder. Proxy and REC are selected independently in the UI
(`encode_preset` vs `record_preset`).

## Later

| Codec | Status | Plan |
|-------|--------|------|
| ProRes 4444 | encoder supports | Optional if needed |
| True Sony XAVC | needs vendor tooling | Removed open-source approx; revisit only with licensed tooling |
| AV1 NVENC | P2000 unlikely | Skip |
| **TC interlaced SRT/WebRTC** | done via deinterlace | DeckLink OUT stays interlaced; proxy encode deinterlaces before NVENC so SRT + WebRTC are progressive |
| DNxHR / 4:4:4 | not started | UHD / 444 mezz |

## Dual REC + audio

Proxy REC and encoded HQ each attach their own AAC from tee `a` (`voaacenc` per
stereo pair). Running **both** encoded roles at once is rejected — use mezz HQ
(`dnxhd_*` / `prores_*`, PCM) together with proxy instead.

## Deferred audio

Proxy/SRT 8ch is four AAC stereo pairs in one MPEG-TS program (not one 8ch AAC
stream). Players that only open the first audio PID will still look like 2ch.
Mezz uses PCM (stereo or 8ch as four stereo pairs) for DNxHD (MXF) and ProRes (MOV).
