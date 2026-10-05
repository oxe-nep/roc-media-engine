# Cutover from roc-recording

Status as of 2026-10-05.

## Freed from roc-recording

| Area | Status |
|------|--------|
| Capture/REC/SRT engine | `roc-media-engine` systemd on `:8080` |
| Go backend | inactive / must stay off |
| Frontend image | `ghcr.io/oxe-nep/roc-media-engine-frontend:latest` |
| k3s Deployment/Service/Secret/IngressRoute | `roc-media-engine-*` |
| Config path defaults | `/opt/applications/roc-media-engine/{hls,recordings}` |

## Intentionally kept

| Item | Why |
|------|-----|
| Public DNS `recording.nepsweden.tech` | Existing certs / bookmarks |
| Physical library dir `/opt/application/roc-recording/backend/recordings` | Existing media; exposed via symlink |
| Physical HLS dir `/opt/applications/roc-recording/backend/hls` | Live preview tree; exposed via symlink |
| Commentator k3s resources `roc-recording-commentator*` | Separate app until WebRTC lands on engine |
| Browser event name `roc-recording-state` | Cosmetic only |

## NTP (timecode)

Capture host chrony uses facility NTP (not public Ubuntu pools):

- `10.199.6.10` / `10.199.6.14`
- Helper: [`deploy/remote-ntp-roc.sh`](../deploy/remote-ntp-roc.sh)

Mezz REC stamps frames via `timecodestamper source=rtc` (see [CODECS.md](CODECS.md)).


## Host layout

```text
/opt/applications/roc-media-engine/
  config.yaml          # recordings_dir + hls_dir under this tree
  recordings -> /opt/application/roc-recording/backend/recordings
  hls        -> /opt/applications/roc-recording/backend/hls
```

Path cutover helper: [`deploy/remote-path-cutover.sh`](../deploy/remote-path-cutover.sh).

## Historical scripts

Go-patch / dual-stack helpers live under [`deploy/archive/`](../deploy/archive/) and are not used in production.

## Verify results

Ran 2026-10-05 via [`deploy/remote-verify-schedule-library.sh`](../deploy/remote-verify-schedule-library.sh) on ch4 — **PASS=9 FAIL=0**.

| Check | Result | Notes |
|-------|--------|-------|
| Library categories | PASS | `_unsorted` and others listed |
| Library file serve | PASS | 200, ~5.9 MB sample |
| Library move | PASS | round-trip `_unsorted` ↔ `_verify_cutover_tmp` |
| Schedule start/stop | PASS | started ~15s after set; stopped at `stop_at` |
