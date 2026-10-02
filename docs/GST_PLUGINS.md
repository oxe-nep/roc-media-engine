# GStreamer plugin requirements

Target: Ubuntu/Debian capture host with Blackmagic Desktop Video + NVIDIA proprietary driver.

## Packages (indicative)

```bash
sudo apt install \
  gstreamer1.0-tools \
  gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav \
  libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev
```

DeckLink elements require **gst-plugins-bad built against the Blackmagic DeckLink SDK**. Distro packages often **omit** DeckLink — you may need a custom build (same class of problem as custom FFmpeg+decklink today).

NVIDIA encode: prefer `gstreamer1.0-plugins-bad` with nvcodec, or NVIDIA’s GST nvcodec binaries. Inspect:

```bash
gst-inspect-1.0 nvh264enc
gst-inspect-1.0 nvautogpuh264enc
gst-inspect-1.0 decklinkvideosrc
gst-inspect-1.0 srtsink
```

## Rust build env

```bash
export PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig
cargo build -p roc-engine --release
```

Windows note

Dev laptops without GStreamer: default `cargo build` uses the mock backend.
On the capture host: `cargo build --release --features gst`.
