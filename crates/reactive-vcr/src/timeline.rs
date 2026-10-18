//! The frame size and time range selected from a source video, and the audio features for it.

use crate::audio::features::{FeatureParams, FeatureTimeline, SAMPLE_RATE, extract, frames_for};
use crate::mapping::{CompiledMapping, Normalize};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// What the effect needs to know about a source video.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Constant output frame rate as a fraction.
    pub fps_n: i32,
    pub fps_d: i32,
    pub duration_s: f64,
    pub has_audio: bool,
}

impl VideoInfo {
    pub fn fps(&self) -> f64 {
        self.fps_n as f64 / self.fps_d as f64
    }
}

/// Validate the user-selected scale and time range before touching any media.
pub fn validate_timeline(
    preview: Option<f64>,
    start_s: Option<f64>,
    duration_s: Option<f64>,
) -> Result<()> {
    if preview.is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1.0) {
        bail!("--preview must be finite and within (0, 1]");
    }
    if start_s.is_some_and(|value| !value.is_finite() || value < 0.0) {
        bail!("--start must be finite and nonnegative");
    }
    if duration_s.is_some_and(|value| !value.is_finite() || value <= 0.0) {
        bail!("--duration must be finite and positive");
    }
    Ok(())
}

/// `dim * scale`, rounded down to an even size of at least 2.
pub fn even_scaled(dim: u32, scale: f64) -> u32 {
    let scaled = ((dim as f64 * scale) / 2.0).floor() as u32 * 2;
    scaled.max(2)
}

/// Frame size and time range selected from the source video.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selection {
    pub width: u32,
    pub height: u32,
    pub fps_n: i32,
    pub fps_d: i32,
    pub start_s: f64,
    pub duration_s: f64,
    pub frames: usize,
}

impl Selection {
    pub fn fps(&self) -> f64 {
        self.fps_n as f64 / self.fps_d as f64
    }
}

pub fn select_timeline(
    info: &VideoInfo,
    preview: Option<f64>,
    start_s: Option<f64>,
    duration_s: Option<f64>,
) -> Result<Selection> {
    validate_timeline(preview, start_s, duration_s)?;
    let scale = preview.unwrap_or(1.0);
    let start_s = start_s.unwrap_or(0.0);
    let video_span = (info.duration_s - start_s).max(0.0);
    let duration_s = duration_s.map_or(video_span, |duration| duration.min(video_span));
    if duration_s <= 0.0 {
        bail!(
            "selected video range is empty: start {start_s}s is at or beyond {}s",
            info.duration_s
        );
    }
    Ok(Selection {
        width: even_scaled(info.width, scale),
        height: even_scaled(info.height, scale),
        fps_n: info.fps_n,
        fps_d: info.fps_d,
        start_s,
        duration_s,
        frames: (duration_s * info.fps()).round() as usize,
    })
}

/// Audio features for every selected video frame, from mono samples at `SAMPLE_RATE`.
pub fn selection_features(
    samples: &[f32],
    map: &CompiledMapping,
    selection: &Selection,
) -> Result<FeatureTimeline> {
    let fps = selection.fps();
    let Selection {
        start_s,
        duration_s,
        frames,
        ..
    } = *selection;
    if frames_for(samples.len(), SAMPLE_RATE, fps, start_s, Some(duration_s)) == 0 {
        bail!("audio file has no content after start offset {start_s}s");
    }
    Ok(extract(
        samples,
        SAMPLE_RATE,
        &FeatureParams {
            fps,
            frames: frames.max(1),
            start_s,
            bands: map.bands.iter().map(|(_, range)| *range).collect(),
            normalize: map.normalize,
            onset_sensitivity: map.onset_sensitivity,
        },
    ))
}

/// Mapping settings that change the extracted audio features.
#[derive(Debug, Clone, PartialEq)]
struct FeatureKey {
    bands: Vec<[f32; 2]>,
    normalize: Normalize,
    onset_sensitivity: f32,
}

impl FeatureKey {
    fn of(map: &CompiledMapping) -> Self {
        Self {
            bands: map.bands.iter().map(|(_, range)| *range).collect(),
            normalize: map.normalize,
            onset_sensitivity: map.onset_sensitivity,
        }
    }
}

/// Reuses extracted features until a mapping edit changes bands, normalization, or onsets.
#[derive(Debug, Default)]
pub struct FeatureCache {
    cached: Option<(FeatureKey, Arc<FeatureTimeline>)>,
}

impl FeatureCache {
    pub fn features(
        &mut self,
        samples: &[f32],
        map: &CompiledMapping,
        selection: &Selection,
    ) -> Result<Arc<FeatureTimeline>> {
        let key = FeatureKey::of(map);
        match &self.cached {
            Some((current, features)) if *current == key => Ok(features.clone()),
            _ => {
                let features = Arc::new(selection_features(samples, map, selection)?);
                self.cached = Some((key, features.clone()));
                Ok(features)
            }
        }
    }
}

fn canonical_existing(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn paths_alias(left: &Path, right: &Path) -> bool {
    if canonical_existing(left)
        .zip(canonical_existing(right))
        .is_some_and(|(left, right)| left == right)
    {
        return true;
    }
    #[cfg(unix)]
    {
        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }
    false
}

/// Refuse an output path that is, links to, or hard-links any input.
pub fn validate_output_is_distinct(output: &Path, inputs: &[(&str, &Path)]) -> Result<()> {
    for &(label, input) in inputs {
        if paths_alias(output, input) {
            bail!(
                "output {} aliases {label} input {}; choose a distinct output path",
                output.display(),
                input.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(duration_s: f64) -> VideoInfo {
        VideoInfo {
            width: 641,
            height: 480,
            fps_n: 30_000,
            fps_d: 1001,
            duration_s,
            has_audio: false,
        }
    }

    #[test]
    fn even_scaling() {
        assert_eq!(even_scaled(641, 1.0), 640);
        assert_eq!(even_scaled(480, 0.5), 240);
        assert_eq!(even_scaled(3, 0.1), 2);
    }

    #[test]
    fn selection_clamps_duration_to_the_video() {
        let selection = select_timeline(&info(10.0), Some(0.5), Some(8.0), Some(5.0)).unwrap();
        assert_eq!((selection.width, selection.height), (320, 240));
        assert_eq!(selection.duration_s, 2.0);
        assert_eq!(selection.frames, 60);
    }

    #[test]
    fn selection_rejects_empty_and_invalid_ranges() {
        let empty = select_timeline(&info(10.0), None, Some(10.0), None).unwrap_err();
        assert!(format!("{empty:#}").contains("empty"));
        for (preview, start, duration, flag) in [
            (Some(0.0), None, None, "--preview"),
            (Some(1.5), None, None, "--preview"),
            (None, Some(-1.0), None, "--start"),
            (None, Some(f64::NAN), None, "--start"),
            (None, None, Some(0.0), "--duration"),
            (None, None, Some(f64::INFINITY), "--duration"),
        ] {
            let error = select_timeline(&info(10.0), preview, start, duration).unwrap_err();
            assert!(format!("{error:#}").contains(flag), "{flag}: {error:#}");
        }
    }

    #[test]
    fn outputs_may_not_alias_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.mp4");
        std::fs::write(&input, "keep").unwrap();
        let fresh = dir.path().join("out.mp4");
        assert!(validate_output_is_distinct(&fresh, &[("video", &input)]).is_ok());
        let mut aliases = vec![input.clone()];
        // Hard links are only detected on Unix, where `paths_alias` compares inodes.
        if cfg!(unix) {
            let link = dir.path().join("link.mp4");
            std::fs::hard_link(&input, &link).unwrap();
            aliases.push(link);
        }
        for output in &aliases {
            let error = validate_output_is_distinct(output, &[("video", &input)]).unwrap_err();
            assert!(format!("{error:#}").contains("aliases video input"));
        }
    }
}
