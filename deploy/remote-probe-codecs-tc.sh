#!/usr/bin/env bash
set +e
echo '=== NTP / chrony / timedatectl ==='
timedatectl status 2>/dev/null | head -25
echo '---'
systemctl is-active chrony chronyd systemd-timesyncd ntp ntpd 2>/dev/null
chronyc tracking 2>/dev/null | head -15
timedatectl show-timesync 2>/dev/null | head -20
echo '=== GST elements ==='
for e in avenc_dnxhd avdec_dnxhd avenc_mpeg2video avenc_prores avenc_prores_ks timecodestamper clockoverlay timeoverlay qtmux mp4mux matroskamux avmux_mov avmux_mxf mxfmux nvh264enc nvh265enc x264enc; do
  if gst-inspect-1.0 "$e" >/dev/null 2>&1; then echo "OK $e"; else echo "MISS $e"; fi
done
echo '=== search ==='
gst-inspect-1.0 2>/dev/null | grep -iE 'dnx|prores|mxf|timecode|avenc_|nvh264|nvh265' | head -80
echo '=== ffmpeg encoders ==='
ffmpeg -hide_banner -encoders 2>/dev/null | grep -iE 'dnx|mpeg2|prores|nvenc' | head -40
echo '=== ffmpeg muxers ==='
ffmpeg -hide_banner -muxers 2>/dev/null | grep -iE 'mxf|mov|mp4' | head -20
echo '=== nvidia ==='
nvidia-smi -L 2>/dev/null | head -5
echo '=== avenc_dnxhd props ==='
gst-inspect-1.0 avenc_dnxhd 2>/dev/null | head -80
echo '=== timecodestamper ==='
gst-inspect-1.0 timecodestamper 2>/dev/null | head -80
