use crate::audio::decode::{SAMPLE_RATE, decode_mono_f32};
use crate::audio::features::{FeatureParams, FeatureTimeline, extract, frames_for};
use crate::mapping;
use crate::modulate::Modulator;
use crate::params::ParamCatalog;
use crate::render::validate_output_is_distinct;
use anyhow::{Context as _, Result, bail};
use std::io::Write;
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

fn validate_timeline(o: &AnalyzeOptions) -> Result<()> {
    if !o.fps.is_finite() || o.fps <= 0.0 {
        bail!("--fps must be finite and > 0");
    }
    if o.start_s
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        bail!("--start must be finite and nonnegative");
    }
    if o.duration_s
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        bail!("--duration must be finite and positive");
    }
    Ok(())
}

fn write_csv_field<W: Write>(w: &mut W, value: &str) -> Result<()> {
    if value.contains([',', '"', '\n', '\r']) {
        write!(w, "\"{}\"", value.replace('"', "\"\""))?;
    } else {
        write!(w, "{value}")?;
    }
    Ok(())
}

pub fn write_csv<W: Write>(
    mut w: W,
    fps: f64,
    start_s: f64,
    band_names: &[String],
    target_names: &[&str],
    features: &FeatureTimeline,
    values: &[Vec<f64>],
) -> Result<()> {
    write!(w, "frame,time,rms")?;
    for band_name in band_names {
        write!(w, ",")?;
        write_csv_field(&mut w, &format!("band_{band_name}"))?;
    }
    write!(w, ",onset")?;
    for target_name in target_names {
        write!(w, ",")?;
        write_csv_field(&mut w, target_name)?;
    }
    writeln!(w)?;
    for frame in 0..features.frames {
        let time = start_s + (frame as f64 + 0.5) / fps;
        write!(w, "{frame},{time:.4},{:.4}", features.rms[frame])?;
        for band in &features.bands {
            write!(w, ",{:.4}", band[frame])?;
        }
        write!(w, ",{:.4}", features.onset[frame])?;
        for value in &values[frame] {
            write!(w, ",{value}")?;
        }
        writeln!(w)?;
    }
    w.flush()?;
    Ok(())
}

pub fn analyze(o: &AnalyzeOptions) -> Result<()> {
    validate_timeline(o)?;
    validate_output_is_distinct(&o.output, &[("audio", &o.audio), ("mapping", &o.map)])?;

    let catalog = ParamCatalog::new();
    let map = mapping::load(&o.map, &catalog)?;
    if let Some(preset) = &map.preset_path {
        validate_output_is_distinct(&o.output, &[("preset", preset)])?;
    }
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
    let mut modulator = Modulator::new(&catalog, &map, &features, o.fps);
    let target_names: Vec<_> = modulator
        .target_indices()
        .iter()
        .map(|&index| catalog.param(index).name)
        .collect();
    let mut values = Vec::with_capacity(frames);
    for frame in 0..frames {
        modulator.effect_for_frame(frame);
        values.push(modulator.last_values().to_vec());
    }
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
    eprintln!(
        "wrote {frames} frames of features to {}",
        o.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::decode::tool_available;
    use crate::audio::features::FeatureTimeline;
    use std::io::{self, Write};
    use std::process::Command;

    #[test]
    fn csv_has_header_and_one_row_per_frame() {
        let features = FeatureTimeline {
            frames: 2,
            rms: vec![0.1, 0.2],
            bands: vec![vec![0.3, 0.4]],
            onset: vec![0.0, 1.0],
        };
        let values = vec![vec![5.0], vec![6.0]];
        let mut output = Vec::new();

        write_csv(
            &mut output,
            30.0,
            1.0,
            &["bass".to_string()],
            &["snow_intensity"],
            &features,
            &values,
        )
        .unwrap();

        let lines: Vec<_> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines[0], "frame,time,rms,band_bass,onset,snow_intensity");
        assert_eq!(lines[1], "0,1.0167,0.1000,0.3000,0.0000,5");
        assert_eq!(lines[2], "1,1.0500,0.2000,0.4000,1.0000,6");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn csv_quotes_arbitrary_header_names() {
        let features = FeatureTimeline {
            frames: 1,
            rms: vec![0.0],
            bands: vec![vec![0.0]],
            onset: vec![0.0],
        };
        let mut output = Vec::new();

        write_csv(
            &mut output,
            1.0,
            0.0,
            &["low,\"wide\"\nband".to_string()],
            &["target,one"],
            &features,
            &[vec![0.0]],
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "frame,time,rms,\"band_low,\"\"wide\"\"\nband\",onset,\"target,one\"\n0,0.5000,0.0000,0.0000,0.0000,0\n"
        );
    }

    struct FlushFails;

    impl Write for FlushFails {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[test]
    fn csv_propagates_flush_error() {
        let features = FeatureTimeline {
            frames: 0,
            rms: vec![],
            bands: vec![],
            onset: vec![],
        };

        let error = write_csv(FlushFails, 30.0, 0.0, &[], &[], &features, &[]).unwrap_err();

        assert!(format!("{error:#}").contains("flush failed"));
    }

    #[test]
    fn analyze_rejects_nonfinite_timeline_before_decoding() {
        let base = AnalyzeOptions {
            audio: "/missing/audio.wav".into(),
            map: "/missing/map.toml".into(),
            fps: 30.0,
            output: "/missing/features.csv".into(),
            start_s: None,
            duration_s: None,
        };
        for fps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let error = analyze(&AnalyzeOptions {
                fps,
                ..base.clone()
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("--fps"));
        }
        for start_s in [Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let error = analyze(&AnalyzeOptions {
                start_s,
                ..base.clone()
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("--start"));
        }
        for duration_s in [Some(-1.0), Some(0.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let error = analyze(&AnalyzeOptions {
                duration_s,
                ..base.clone()
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
                fps: 30.0,
                output: output.clone(),
                start_s: None,
                duration_s: None,
            })
            .unwrap_err();

            assert!(format!("{error:#}").contains(&format!("aliases {kind} input")));
            assert_eq!(std::fs::read_to_string(output).unwrap(), original);
        }
    }

    #[test]
    fn analyze_decodes_audio_and_writes_per_frame_csv() {
        if !tool_available("ffmpeg") {
            eprintln!("skip: ffmpeg missing");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("tone.wav");
        let output = dir.path().join("features.csv");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=110:duration=1",
            ])
            .arg(&audio)
            .status()
            .unwrap();
        assert!(status.success());

        analyze(&AnalyzeOptions {
            audio,
            map: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/basic.toml"),
            fps: 4.0,
            output: output.clone(),
            start_s: None,
            duration_s: Some(0.5),
        })
        .unwrap();

        let lines: Vec<_> = std::fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "frame,time,rms,band_bass,band_high,onset,tracking_noise,tracking_noise_wave_intensity,tracking_noise_height,head_switching,head_switching_horizontal_shift,snow_intensity,chroma_noise_intensity"
        );
        assert!(lines[1].starts_with("0,0.1250,"));
        assert!(lines[2].starts_with("1,0.3750,"));
    }
}
