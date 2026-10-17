//! Video stream facts via the GStreamer Discoverer.

use anyhow::{Context as _, Result, bail};
use gstreamer as gst;
use reactive_vcr::timeline::VideoInfo;
use std::path::Path;

/// Frame rate used when a stream reports none (variable-rate or still sources).
const FALLBACK_FPS: (i32, i32) = (30, 1);

pub fn file_uri(path: &Path) -> Result<String> {
    if !path.exists() {
        bail!("file not found: {}", path.display());
    }
    let absolute =
        std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    Ok(gst::glib::filename_to_uri(&absolute, None)
        .with_context(|| format!("converting {} to a URI", absolute.display()))?
        .to_string())
}

/// Size, constant frame rate, duration, and audio presence of the first video stream.
pub fn probe(path: &Path) -> Result<VideoInfo> {
    crate::init()?;
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(30))
        .context("creating the GStreamer discoverer")?;
    let info = discoverer
        .discover_uri(&file_uri(path)?)
        .with_context(|| format!("probing {}", path.display()))?;
    let Some(video) = info.video_streams().into_iter().next() else {
        bail!("{} has no video stream", path.display());
    };
    let rate = video.framerate();
    let (fps_n, fps_d) = if rate.numer() > 0 && rate.denom() > 0 {
        (rate.numer(), rate.denom())
    } else {
        FALLBACK_FPS
    };
    let duration_s = info
        .duration()
        .map(|duration| duration.nseconds() as f64 / 1e9)
        .filter(|duration| *duration > 0.0)
        .with_context(|| format!("{} reports no duration", path.display()))?;
    Ok(VideoInfo {
        width: video.width(),
        height: video.height(),
        fps_n,
        fps_d,
        duration_s,
        has_audio: !info.audio_streams().is_empty(),
    })
}
