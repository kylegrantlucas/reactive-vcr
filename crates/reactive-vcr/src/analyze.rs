//! Per-frame audio features and modulated parameter values as CSV.

use crate::audio::features::FeatureTimeline;
use crate::mapping::CompiledMapping;
use crate::modulate::Modulator;
use crate::params::ParamCatalog;
use anyhow::Result;
use std::io::Write;

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

/// Each modulation target's name and its value on every feature frame, as `render` applies them.
pub fn target_values(
    catalog: &ParamCatalog,
    map: &CompiledMapping,
    features: &FeatureTimeline,
    fps: f64,
) -> (Vec<&'static str>, Vec<Vec<f64>>) {
    let mut modulator = Modulator::new(catalog, map, fps);
    let names = modulator
        .target_indices()
        .iter()
        .map(|&index| catalog.param(index).name)
        .collect();
    let values = (0..features.frames)
        .map(|frame| {
            modulator.effect(catalog, &map.base, |source| features.value(source, frame));
            modulator.last_values().to_vec()
        })
        .collect();
    (names, values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

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
}
