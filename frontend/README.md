# roc-media-engine frontend

Next.js dashboard for the Rust media engine. Originally copied from
`roc-recording/frontend`; this repo is now the sole greenfield stack.

## Local

```bash
cp .env.example .env.local
npm ci
npm run dev
```

Point `NEXT_PUBLIC_BACKEND_URL` at the engine (`:8080`). In k3s, leave it empty
so the browser uses same-origin; nginx proxies to `BACKEND_HOST:BACKEND_PORT`.

## k3s

See [`../deploy/k8s-frontend.yaml`](../deploy/k8s-frontend.yaml).

- Image: `ghcr.io/oxe-nep/roc-media-engine-frontend:latest`
- Resources: `roc-media-engine-frontend` / `roc-media-engine-secrets`
- Public host: `recording.nepsweden.tech`
