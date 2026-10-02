# UI / Go adapter (Fas 5)

`roc-recording` stays the production dashboard until this engine is proven.

## Option A — Go proxy (recommended during migration)

In the legacy Go backend, per channel:

```go
// pseudo
if runtime.ChannelUsesMediaEngine(id) {
    return mediaengine.Client.StartCapture(ctx, id)
}
return captureManager.Start(id)
```

Env:

```
MEDIA_ENGINE_URL=http://127.0.0.1:8090
MEDIA_ENGINE_CHANNELS=1   # comma-separated ids on the new stack
```

Status WS: merge `GET {MEDIA_ENGINE_URL}/api/channels` into dashboard snapshots (field `backend: "media_engine" | "ffmpeg"`).

See live contract: `GET /api/adapter/manifest`.

## Option B — Point Next.js at the new API

When all encode/REC/SRT/playout channels are on the engine, set frontend `BACKEND_HOST` / API base to the engine and retire Go managers.

## Workflow stubs

`POST /api/workflows/{channel_id}` with `{"kind":"tc","active":true}` refuses if capture is running (exclusive). Real TC/commentator graphs are Fas 4 follow-ups; stubs exist so UI can wire feature flags early.
