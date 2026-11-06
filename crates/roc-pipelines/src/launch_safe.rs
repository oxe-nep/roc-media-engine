//! Guards for values interpolated into `gst_parse_launch` strings.
//!
//! Launch descriptions quote paths/URIs as `location="…"` / `uri="…"`. A value
//! containing `"` or `\` can close the quote and splice arbitrary elements into
//! the pipeline (e.g. a `filesink` writing anywhere as the service user). Every
//! externally influenced value must pass these checks before it reaches a
//! launch string.

use anyhow::{bail, Result};

const MAX_LEN: usize = 4096;

/// Reject values that could escape a double-quoted launch-string property.
pub fn validate_launch_value(value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("empty media source");
    }
    if value.len() > MAX_LEN {
        bail!("media source too long");
    }
    if value.starts_with('-') {
        bail!("media source must not start with '-'");
    }
    if let Some(c) = value
        .chars()
        .find(|c| matches!(c, '"' | '\\') || c.is_control())
    {
        bail!("media source contains forbidden character {c:?}");
    }
    Ok(())
}

/// Characters allowed in an SRT URL or `host:port` target (plus query string).
fn srt_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            ':' | '/' | '.' | '?' | '&' | '=' | '_' | '-' | '%' | '[' | ']' | '@' | '~' | ',' | '+'
        )
}

/// Validate an SRT caller target (`host:port` or full `srt://…` URL).
pub fn validate_srt_target(target: &str) -> Result<()> {
    let target = target.trim();
    if target.is_empty() {
        bail!("SRT target is empty");
    }
    if target.len() > 1024 {
        bail!("SRT target too long");
    }
    if let Some(c) = target.chars().find(|c| !srt_char_ok(*c)) {
        bail!("SRT target contains forbidden character {c:?}");
    }
    if target.contains("://") && !target.starts_with("srt://") {
        bail!("only srt:// URLs are supported");
    }
    Ok(())
}

/// Validate a playout source: either an `srt://` URL or a filesystem path.
pub fn validate_playout_source(source: &str) -> Result<()> {
    if source.starts_with("srt://") {
        validate_srt_target(source)
    } else if source.contains("://") {
        bail!("unsupported playout source scheme")
    } else {
        validate_launch_value(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_pass() {
        assert!(validate_launch_value("/data/rec/_unsorted/Cam 1_ch1_20260101.mp4").is_ok());
        assert!(validate_playout_source("/opt/x/playout-media/abc_clip.mxf").is_ok());
    }

    #[test]
    fn quote_breakout_rejected() {
        let evil = "/tmp/a.mp4\" ! fakesink filesrc location=\"/etc/shadow";
        assert!(validate_launch_value(evil).is_err());
        assert!(validate_playout_source(evil).is_err());
        assert!(validate_launch_value("a\\\"b").is_err());
        assert!(validate_launch_value("a\nb").is_err());
        assert!(validate_launch_value("-version").is_err());
        assert!(validate_launch_value("").is_err());
    }

    #[test]
    fn srt_targets() {
        assert!(validate_srt_target("10.0.0.5:9000").is_ok());
        assert!(validate_srt_target("srt://host.example:9000?mode=caller&latency=120000").is_ok());
        assert!(validate_srt_target("srt://h:1?x=\" ! filesink location=/x").is_err());
        assert!(validate_srt_target("h:1 ! fakesink").is_err());
        assert!(validate_playout_source("file:///etc/passwd").is_err());
        assert!(validate_playout_source("http://x/y").is_err());
    }
}
