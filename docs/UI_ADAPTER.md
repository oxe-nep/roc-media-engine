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

- Remote commentator WebRTC bridge (workflow option hidden in UI; snapshot `commentator: []`)
- **Native interlaced encode** — today capture always runs `deinterlace` before NVENC, so SRT/REC/proxy are progressive even when DeckLink locks `1080i50`. Goal: keep source scan type through encode (PAFF/MBAFF or field-aware path) with optional deinterlace; decode OUT should then match source rather than forcing `Hi50` → `1080p50`.

Mezz REC (DNxHD / ProRes) and TC burn-in are implemented — see [CODECS.md](CODECS.md) and [CUTOVER.md](CUTOVER.md).
