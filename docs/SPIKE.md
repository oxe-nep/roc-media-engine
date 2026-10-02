# Fas 0 — Spike go / no-go

Run on the **capture host** (Linux, DeckLink Desktop Video, NVIDIA driver, GStreamer with DeckLink + nvcodec).

## Checklist

1. **Plugins present**

```bash
./scripts/check-gst-plugins.sh
# or:
cargo run -p spike-list-devices --release
```

Required elements: `decklinkvideosrc`, `decklinkvideosink`, `nvh264enc` (or `nvautogpuh264enc`), `srtsink`, `mpegtsmux`, `mp4mux`.

2. **Encode 60s (then 30+ min)**

```bash
cargo run -p spike-decklink-nvenc --release -- \
  --device "DeckLink IP 100G (1)" \
  --output /tmp/roc-spike.mp4 \
  --preview \
  --duration-secs 60
```

Then `--duration-secs 1800` for soak.

3. **Signal loss**

Pull ST 2110 source or disable input. Expect bus warning / `waiting` — **not** host process death. Document observed messages in the table below.

4. **Decision**

| Outcome | Action |
|---------|--------|
| DeckLink + NVENC + tee stable | **GO** → Fas 1 on channel 1 in parallel with roc-recording |
| `decklinkvideosrc` missing / broken for IP | Try BMD SDK custom source **or** libav decklink source wrapped as GST `appsrc` |
| NVENC element missing | Install gst nvcodec / DeepStream plugins; fallback `x264enc` only for lab |
| Random whole-process SIGSEGV in BMD | Isolate DeckLink I/O in a supervised subprocess **only for source**, keep encode in-process |

## Observed notes (fill on host)

| Date | Host | Result | Notes |
|------|------|--------|-------|
| | | | |

## Manual gst-launch

```bash
cargo run -p spike-decklink-nvenc -- --device "DeckLink IP 100G (1)" --preview
# prints gst-launch-1.0 line
```
