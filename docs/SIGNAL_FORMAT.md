# Input format detection

Set per channel in config:

```yaml
mode: auto      # probe on start + re-lock when caps change (recommended)
mode: 1080p50   # force lock (skip probe)
mode: 1080i50
```

API channel snapshots include `configured_mode`, `locked_mode`, and `input_format`.

Why not leave `decklinkvideosrc mode=auto` forever? Auto renegotiation (e.g. brief SD then HD) breaks the encode graph. We probe, lock a concrete mode, then watch live caps and relaunch only when the real format changes.
