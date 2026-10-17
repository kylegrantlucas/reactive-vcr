//! `analyze`: decode an audio file and write its per-frame features and target values as CSV.

use crate::decode::decode_mono_f32;
use anyhow::{Context as _, Result, bail};
use reactive_vcr::analyze::{target_values, write_csv};
use reactive_vcr::audio::features::{FeatureParams, SAMPLE_RATE, extract, frames_for};
use reactive_vcr::mapping;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::{validate_output_is_distinct, validate_timeline};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    pub audio: PathBuf,
    pub map: PathBuf,
    pub fps: f64,
    pub output: PathBuf,
    pub start_s: Option<f64>,
    pub duration_s: Option<f64>,
}

/// Returns the number of frames written.
pub fn analyze(o: &AnalyzeOptions) -> Result<usize> {
    if !o.fps.is_finite() || o.fps <= 0.0 {
        bail!("--fps must be finite and > 0");
    }
    validate_timeline(None, o.start_s, o.duration_s)?;
    validate_output_is_distinct(&o.output, &[("audio", &o.audio), ("mapping", &o.map)])?;

    let catalog = ParamCatalog::new();
    let map = mapping::load(&o.map, &catalog)?;
    validate_output_is_distinct(&o.output, &map.preset_inputs())?;
    let samples = decode_mono_f32(&o.audio)?;
    let start_s = o.start_s.unwrap_or(0.0);
    let frames = frames_for(samples.len(), SAMPLE_RATE, o.fps, start_s, o.duration_s);
    if frames == 0 {
        bail!("no audio after start offset {start_s}s");
    }
    let features = extract(
        &samples,
        SAMPLE_RATE,
        &FeatureParams {
            fps: o.fps,
            frames,
            start_s,
            bands: map.bands.iter().map(|(_, range)| *range).collect(),
            normalize: map.normalize,
            onset_sensitivity: map.onset_sensitivity,
        },
    );
    let (target_names, values) = target_values(&catalog, &map, &features, o.fps);
    let file = std::fs::File::create(&o.output)
        .with_context(|| format!("creating {}", o.output.display()))?;
    let band_names: Vec<_> = map.bands.iter().map(|(name, _)| name.clone()).collect();
    write_csv(
        std::io::BufWriter::new(file),
        o.fps,
        start_s,
        &band_names,
        &target_names,
        &features,
        &values,
    )?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> AnalyzeOptions {
        AnalyzeOptions {
            audio: "/missing/audio.wav".into(),
            map: "/missing/map.toml".into(),
            fps: 30.0,
            output: "/missing/features.csv".into(),
            start_s: None,
            duration_s: None,
        }
    }

    #[test]
    fn analyze_rejects_nonfinite_timeline_before_decoding() {
        for fps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let error = analyze(&AnalyzeOptions { fps, ..options() }).unwrap_err();
            assert!(format!("{error:#}").contains("--fps"));
        }
        for start_s in [Some(-1.0), Some(f64::NAN)] {
            let error = analyze(&AnalyzeOptions {
                start_s,
                ..options()
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("--start"));
        }
        for duration_s in [Some(0.0), Some(f64::INFINITY)] {
            let error = analyze(&AnalyzeOptions {
                duration_s,
                ..options()
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("--duration"));
        }
    }

    #[test]
    fn analyze_rejects_an_output_that_aliases_an_input() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("audio.wav");
        let map = dir.path().join("map.toml");
        std::fs::write(&audio, "audio must not be overwritten").unwrap();
        std::fs::write(&map, "mapping must not be overwritten").unwrap();
        for (kind, output, original) in [
            ("audio", audio.clone(), "audio must not be overwritten"),
            ("mapping", map.clone(), "mapping must not be overwritten"),
        ] {
            let error = analyze(&AnalyzeOptions {
                audio: audio.clone(),
                map: map.clone(),
                output: output.clone(),
                ..options()
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains(&format!("aliases {kind} input")));
            assert_eq!(std::fs::read_to_string(output).unwrap(), original);
        }
    }
}
