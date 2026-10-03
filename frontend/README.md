# roc-media-engine frontend

Next.js dashboard for the Rust media engine. Copied from `roc-recording/frontend`
so both repos stay complete: legacy Go stack there, greenfield stack here.

## Local

```bash
cp .env.example .env.local
npm ci
npm run dev
```

Point `NEXT_PUBLIC_BACKEND_URL` at the engine (`:8080`). In k3s, leave it empty
so the browser uses same-origin; nginx proxies to `BACKEND_HOST:BACKEND_PORT`.

## k3s

See [`../deploy/k8s-frontend.yaml`](../deploy/k8s-frontend.yaml). Image name can
stay `ghcr.io/oxe-nep/roc-recording-frontend` until a dedicated GHCR repo exists.
