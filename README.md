# roc-media-engine

Greenfield media stack for ROC live capture/record/SRT/playout.

**Replaces FFmpeg child-process orchestration** (as in `roc-recording`) with an **in-process Rust + GStreamer** engine: one host process, bus-based errors, `tee` fan-out, NVENC encode-once.

Legacy [`roc-recording`](../roc-recording) stays as the frozen Go+FFmpeg stack.
This repo is the complete greenfield product: **Rust engine + Next.js UI**.

## Goals

- Stability: no orphaned `ffmpeg` processes; structured pipeline errors
- Performance: single NVENC pass, tee to REC / SRT / UDP / preview
- Ops: systemd unit, `/api/health`, NVENC session limits

## Layout

```
roc-media-engine/
  crates/
    roc-config/       # YAML config + encode presets
    roc-devices/      # DeckLink descriptors / format codes
    roc-pipelines/    # GStreamer + mock backends
    roc-engine/       # axum API + orchestrator (bin: roc-media-engine)
  frontend/           # Next.js dashboard (k3s)
  spikes/
    list_devices/     # Fas 0: probe GST elements / devices
    decklink_nvenc/   # Fas 0: DeckLink → NVENC → file (+ tee)
  deploy/             # systemd + k8s + env example
  docs/               # spike go/no-go, plugins, UI cutover
  scripts/            # host helper scripts
```

## Quick start (dev without DeckLink / GStreamer)

Default build uses the **mock** backend (no GStreamer link). The engine serves the
UI API on **`:8080`** (same port the old Go backend used). See
[docs/UI_ADAPTER.md](docs/UI_ADAPTER.md) and [frontend/README.md](frontend/README.md).

```bash
cargo run -p roc-engine -- --write-example-config --config config.yaml
cargo run -p roc-engine -- --config config.yaml
curl http://127.0.0.1:8080/api/health
curl -X POST http://127.0.0.1:8080/api/streams/1/start
# UI
cd frontend && cp .env.example .env.local && npm ci && npm run dev
```

## Capture host (Linux + DeckLink IP + NVIDIA)

Enable the real backend with `--features gst` (see [docs/GST_PLUGINS.md](docs/GST_PLUGINS.md), [docs/SPIKE.md](docs/SPIKE.md), [docs/CODECS.md](docs/CODECS.md)).

```bash
cargo build -p roc-engine --release --features gst
cargo run -p spike-list-devices --release --features gst
cargo run -p spike-decklink-nvenc --release --features gst -- \
  --device "DeckLink IP 100G (1)" --preview --duration-secs 60
```

## API (v0.1)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/health` | liveness + NVENC slots |
| GET | `/api/devices` | probe report |
| GET | `/api/channels` | encode channel snapshots |
| POST | `/api/channels/{id}/start\|stop` | capture |
| POST | `/api/channels/{id}/record/start\|stop` | fragmented MP4 |
| POST | `/api/channels/{id}/srt/start\|stop` | SRT branch |
| GET/POST | `/api/playout…` | decode → DeckLink |
| GET/POST | `/api/workflows…` | TC/commentator stubs |
| GET | `/api/adapter/manifest` | Go/UI migration contract |

## SRT

Each channel can publish MPEG-TS over SRT (H.264 + AAC ADTS). Encode presets
default to **stereo** (`audio_channels: 2`). Full 8ch (4×AAC pairs) is deferred
until REC/SRT quality is solid — set `audio_channels: 8` only for experiments.

Default listener bind (config): `srt://0.0.0.0:910N?mode=listener` for channel N.

VLC / ffplay (caller):

```text
srt://<engine-host>:9101?mode=caller&latency=120
…
srt://<engine-host>:9108?mode=caller&latency=120
```

Caller mode (e.g. MediaMTX) is the same encode path — set the URL via
`POST /api/channels/{id}/srt/start` with `{"url":"srt://…"}`.

## Phases

| Fas | Status in tree |
|-----|----------------|
| 0 Spike | `spikes/*` + `docs/SPIKE.md` |
| 1 Capture+REC | engine API + encode-once tee |
| 2 Multi+SRT | 8 channels, NVENC limit, SRT |
| 3 Playout | SRT/file → DeckLink OUT |
| 4–5 Workflows/UI | stubs + adapter manifest |

## License

MIT
