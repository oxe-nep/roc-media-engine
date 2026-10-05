# UI + engine (cutover B)

`roc-media-engine` is the complete greenfield stack:

| Piece | Location |
|-------|----------|
| Backend | this repo (`roc-engine` on `:8080`) |
| Frontend | [`frontend/`](../frontend/) (Next.js) |
| Legacy Go | [`roc-recording`](../../roc-recording) — frozen reference only |

## Dev

```bash
# terminal 1 – engine
cargo run -p roc-engine --features gst -- --config config.yaml

# terminal 2 – UI
cd frontend && cp .env.example .env.local && npm run dev
```

`.env.example` points at `http://10.199.28.249:8080`. In k3s, leave
`NEXT_PUBLIC_BACKEND_URL` empty (same-origin); nginx uses `BACKEND_PORT=8080`.

## Production

- Engine systemd on capture host, bind `0.0.0.0:8080`
- Frontend: [`deploy/k8s-frontend.yaml`](../deploy/k8s-frontend.yaml)
  - Image: `ghcr.io/oxe-nep/roc-media-engine-frontend:latest`
  - Resources: `roc-media-engine-*`
- Public host: `recording.nepsweden.tech`
- Do **not** run Go `roc-recording.service` (port conflict)

See also [CUTOVER.md](CUTOVER.md).

## Still deferred

- TC-loop + commentator/WebRTC on the engine (UI grids still present; engine returns empty stubs)
- DNxHD / XAVC mezz codecs (see [CODECS.md](CODECS.md))
