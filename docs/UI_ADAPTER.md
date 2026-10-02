# UI / Go adapter (Fas 5)

`roc-recording` stays the production dashboard until this engine is proven.

## Option A — Go proxy (recommended during migration)

Implemented in `roc-recording/backend/internal/mediaengine`.

Env on the **Go** backend host (same machine as the engine for thumbs):

```
MEDIA_ENGINE_URL=http://127.0.0.1:8090
MEDIA_ENGINE_CHANNELS=1          # comma-separated; use `all` for every id
MEDIA_ENGINE_PLAYOUT=1           # optional; decode client ids → engine `decode-{id}`
```

Per channel in those IDs, Go proxies:

| UI / Go route | Engine |
|---|---|
| `POST /api/streams/{id}/start\|stop` | `/api/channels/{id}/start\|stop` |
| `POST /api/recordings/{id}/start\|stop` | `/api/channels/{id}/record/start\|stop` |
| `POST /api/srt/{id}/start\|stop` | `/api/channels/{id}/srt/start\|stop` |

Dashboard WS merges engine snapshots and sets `streams[].backend` to `"media_engine"` or `"ffmpeg"`.
Recording: Go builds the absolute path under the UI recordings directory (`RecordingDir()` / `recordings-path.json`) and calls engine with `?path=`.

Rollout: start with `MEDIA_ENGINE_CHANNELS=1`, verify encode/REC/SRT in the UI, then expand.

See live contract: `GET /api/adapter/manifest` on the media engine.

## Option B — Point Next.js at the new API

When all encode/REC/SRT/playout channels are on the engine, set frontend `BACKEND_HOST` / API base to the engine and retire Go managers.

## Workflow stubs

`POST /api/workflows/{channel_id}` with `{"kind":"tc","active":true}` refuses if capture is running (exclusive). Real TC/commentator graphs are Fas 4 follow-ups; stubs exist so UI can wire feature flags early.

## Known gaps (engine channels)

- ~~Scheduled recordings~~ — wired via `SetEngineHooks` (start/stop + signal check)
- ~~Library paths~~ — Go passes UI recordings root via `?path=` (`RecordingDir()` / `recordings-path.json`); engine `recordings_dir` is soak/direct fallback only
- ~~SRT audio~~ — stereo AAC (pair 1–2) muxed into mpegts with video
- ~~SRT bitrate badge~~ — **live** MPEG-TS bitrate from GST pad probe (`srt_bitrate_kbps`); Sending=true only after first sample (no preset fallback)
- ~~Encode preset apply~~ — `POST /api/channels/{id}/encode-preset` + Go `PUT /api/streams/{id}/encode-preset` proxy (snapshot returns preset **id**)
- ~~REC elapsed badge~~ — Go `MarkEngineRecording` + wall-clock elapsed
- ~~SRT GET overlay~~ — `GET /api/srt/{id}` merges engine status for settings modal
- ~~REC/UDP program audio~~ — stereo AAC (pair 1–2) into mp4mux / UDP mpegtsmux (same as SRT)
- Per-channel `/api/recordings/files/{id}` still looks under `recordings/{id}/` (legacy); library UI uses category folders
- ~~Custom Go presets~~ — Go CRUD + apply push full body to engine (`PUT /api/encode/presets/{id}`); codec map `h264_nvenc`→`nvh264enc`, `p4`→`hq`
- EncodePresetsEditor codec list is still FFmpeg-oriented in the UI (`h264_nvenc`); engine normalizes on ingest
- ~~Playout SRT→DeckLink~~ — engine pipeline OK (UYVY); Go proxy via `MEDIA_ENGINE_PLAYOUT=1` (or `all`) → `decode-{id}`; file/pause/resume still FFmpeg-only
- TC / commentator remain on FFmpeg managers (exclusive workflows not on GST yet)
- Playout decode path exists in engine; wire more UI parity (meters/thumbs/file) next
- **Do not scrap Go UI backend** until playout + TC + commentator/WebRTC are on the engine (or a Rust UI API) with feature parity

## When to retire Go for UI

Keep Go as the UI façade while MEDIA_ENGINE covers capture/REC/SRT/preview/meters. Scrap/replace only after:

1. Playout (DeckLink OUT) driven from engine with UI parity
2. TC + commentator/WebRTC workflows on engine (or deliberately dropped)
3. Live encode/SRT stats (bitrate) from GST — **done for SRT badge**; refine as needed
4. Library/schedule/auth already stable through the adapter

Until then: Go adapter + engine side-by-side is the plan — not a temporary accident.