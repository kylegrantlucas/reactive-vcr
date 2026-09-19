#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::get_param;
    use std::path::Path;

    const GOOD: &str = r#"
[base]
seed = 7

[audio]
bands.bass = [20, 200]
bands.high = [3000, 12000]
normalize = "percentile:98"

[[modulate]]
target = "tracking_noise_wave_intensity"
source = "band:bass"
range = [2.0, 30.0]
curve = "pow:2"
attack_ms = 10
release_ms = 300

[[modulate]]
target = "head_switching"
source = "onset"
threshold = 0.6

[[modulate]]
target = "vhs_tape_speed"
source = "rms"
"#;

    fn compile_str(s: &str) -> anyhow::Result<CompiledMapping> {
        let file: MappingFile = toml::from_str(s)?;
        compile(file, Path::new("."), &ParamCatalog::new())
    }

    #[test]
    fn compiles_good_mapping() {
        let m = compile_str(GOOD).unwrap();
        let c = ParamCatalog::new();
        assert_eq!(get_param(&m.base, c.get("random_seed").unwrap()), 7.0);
        assert_eq!(m.bands.len(), 2);
        assert!(matches!(m.normalize, Normalize::Percentile(p) if (p - 98.0).abs() < 1e-6));
        assert_eq!(m.modulations.len(), 3);
        let m0 = &m.modulations[0];
        assert_eq!(c.param(m0.param).name, "tracking_noise_wave_intensity");
        assert!(matches!(m0.source, Source::Band(0)));
        assert_eq!(m0.range, (2.0, 30.0));
        assert_eq!(m0.attack_ms, 10.0);
        let m1 = &m.modulations[1];
        assert!(matches!(m1.source, Source::Onset));
        assert_eq!(m1.threshold, 0.6);
        assert_eq!(m.modulations[2].range, (0.0, 3.0));
    }

    #[test]
    fn reports_all_errors_at_once() {
        let bad = r#"
[[modulate]]
target = "snow_intensty"
source = "rms"

[[modulate]]
target = "snow_intensity"
source = "band:nope"

[[modulate]]
target = "snow_intensity"
source = "rms"
range = [0, 500]

[[modulate]]
target = "head_switching"
source = "rms"
range = [0, 1]

[[modulate]]
target = "snow_intensity"
source = "rms"
curve = "bogus"
"#;
        let err = compile_str(bad).unwrap_err().to_string();
        assert!(err.contains("snow_intensty"), "{err}");
        assert!(err.contains("snow_intensity"), "suggestion missing: {err}");
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("500"), "{err}");
        assert!(err.contains("threshold") || err.contains("bool"), "{err}");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn aggregates_independent_errors_in_one_modulation() {
        let err = compile_str(
            r#"
[[modulate]]
target = "snow_intensity"
source = "band:missing"
range = [nan, 500]
curve = "bogus"
attack_ms = nan
release_ms = -1
"#,
        )
        .unwrap_err()
        .to_string();
        for expected in ["missing", "NaN", "500", "bogus", "attack_ms", "release_ms"] {
            assert!(err.contains(expected), "missing {expected:?} from {err}");
        }
    }

    #[test]
    fn rejects_nonfinite_audio_and_modulation_values() {
        let err = compile_str(
            r#"
[audio]
bands.bad = [nan, inf]
normalize = "percentile:NaN"
onset_sensitivity = nan

[[modulate]]
target = "head_switching"
source = "rms"
threshold = inf
attack_ms = inf
release_ms = nan
"#,
        )
        .unwrap_err()
        .to_string();
        for expected in [
            "band",
            "percentile",
            "onset_sensitivity",
            "threshold",
            "attack_ms",
            "release_ms",
        ] {
            assert!(err.contains(expected), "missing {expected:?} from {err}");
        }
    }

    #[test]
    fn preserves_reversed_numeric_ranges() {
        let mapping = compile_str(
            r#"
[[modulate]]
target = "snow_intensity"
source = "rms"
range = [100, 0]
"#,
        )
        .unwrap();
        assert_eq!(mapping.modulations[0].range, (100.0, 0.0));
    }

    #[test]
    fn loads_preset_relative_to_mapping_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ParamCatalog::new();
        let mut e = ntsc_rs::NtscEffect::default();
        crate::params::set_param(&mut e, c.get("snow_intensity").unwrap(), 42.0).unwrap();
        std::fs::write(dir.path().join("p.json"), c.preset_to_json(&e).unwrap()).unwrap();
        std::fs::write(dir.path().join("m.toml"), "[base]\npreset = \"p.json\"\n").unwrap();
        let m = load(&dir.path().join("m.toml"), &c).unwrap();
        assert_eq!(get_param(&m.base, c.get("snow_intensity").unwrap()), 42.0);
    }

    #[test]
    fn parse_source_variants() {
        let bands = vec![("bass".to_string(), [20.0, 200.0])];
        assert!(matches!(parse_source("rms", &bands).unwrap(), Source::Rms));
        assert!(matches!(
            parse_source("onset", &bands).unwrap(),
            Source::Onset
        ));
        assert!(matches!(
            parse_source("band:bass", &bands).unwrap(),
            Source::Band(0)
        ));
        assert!(parse_source("band:x", &bands).is_err());
        assert!(parse_source("beat", &bands).is_err());
    }
}
use crate::curve::Curve;
use crate::params::{ParamCatalog, set_param};
use anyhow::{Context as _, Result, anyhow, bail};
use ntsc_rs::NtscEffect;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MappingFile {
    pub base: Option<BaseSection>,
    pub audio: Option<AudioSection>,
    #[serde(default)]
    pub modulate: Vec<ModulationSection>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BaseSection {
    pub preset: Option<PathBuf>,
    pub seed: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AudioSection {
    #[serde(default)]
    pub bands: BTreeMap<String, [f32; 2]>,
    pub normalize: Option<String>,
    pub onset_sensitivity: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulationSection {
    pub target: String,
    pub source: String,
    pub range: Option<[f32; 2]>,
    pub threshold: Option<f32>,
    pub curve: Option<String>,
    #[serde(default)]
    pub attack_ms: f32,
    #[serde(default)]
    pub release_ms: f32,
    #[serde(default)]
    pub invert: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Rms,
    Band(usize),
    Onset,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Normalize {
    Peak,
    Percentile(f32),
}

#[derive(Debug, Clone)]
pub struct CompiledModulation {
    /// Index into ParamCatalog.
    pub param: usize,
    pub source: Source,
    pub range: (f32, f32),
    pub threshold: f32,
    pub curve: Curve,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub invert: bool,
}

#[derive(Debug, Clone)]
pub struct CompiledMapping {
    /// Preset input resolved relative to the mapping, retained for output protection.
    pub preset_path: Option<PathBuf>,
    pub base: NtscEffect,
    pub bands: Vec<(String, [f32; 2])>,
    pub normalize: Normalize,
    pub onset_sensitivity: f32,
    pub modulations: Vec<CompiledModulation>,
}

pub fn parse_source(s: &str, bands: &[(String, [f32; 2])]) -> Result<Source> {
    match s.trim() {
        "rms" => Ok(Source::Rms),
        "onset" => Ok(Source::Onset),
        other => {
            if let Some(name) = other.strip_prefix("band:") {
                bands
                    .iter()
                    .position(|(band_name, _)| band_name == name)
                    .map(Source::Band)
                    .ok_or_else(|| {
                        anyhow!(
                            "undefined band {name:?}; defined bands: {}",
                            bands
                                .iter()
                                .map(|(band_name, _)| band_name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
            } else {
                bail!("unknown source {other:?}; expected rms | band:<name> | onset")
            }
        }
    }
}

fn parse_normalize(s: &str) -> Result<Normalize> {
    match s.trim() {
        "peak" => Ok(Normalize::Peak),
        other => {
            let percentile = other.strip_prefix("percentile:").ok_or_else(|| {
                anyhow!("normalize must be \"peak\" or \"percentile:<n>\", got {other:?}")
            })?;
            let percentile: f32 = percentile
                .parse()
                .map_err(|_| anyhow!("percentile needs a number, got {percentile:?}"))?;
            if !percentile.is_finite() || !(0.0..=100.0).contains(&percentile) {
                bail!("percentile must be finite and within 0..=100, got {percentile}");
            }
            Ok(Normalize::Percentile(percentile))
        }
    }
}

pub fn load(path: &Path, catalog: &ParamCatalog) -> Result<CompiledMapping> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading mapping {}", path.display()))?;
    let file =
        toml::from_str(&text).with_context(|| format!("parsing mapping {}", path.display()))?;
    compile(file, path.parent().unwrap_or(Path::new(".")), catalog)
}

pub fn compile(
    file: MappingFile,
    base_dir: &Path,
    catalog: &ParamCatalog,
) -> Result<CompiledMapping> {
    let mut errors = Vec::new();
    let base_section = file.base.unwrap_or_default();
    let mut base = NtscEffect::default();
    let preset_path = base_section.preset.map(|preset| base_dir.join(preset));
    if let Some(full) = &preset_path {
        match std::fs::read_to_string(full)
            .with_context(|| format!("reading preset {}", full.display()))
            .and_then(|json| {
                catalog
                    .preset_from_json(&json)
                    .with_context(|| format!("preset {}", full.display()))
            }) {
            Ok(effect) => base = effect,
            Err(error) => errors.push(error.to_string()),
        }
    }
    if let Some(seed) = base_section.seed {
        let param = catalog.get("random_seed").expect("ntsc-rs has random_seed");
        if let Err(error) = set_param(&mut base, param, seed as f64) {
            errors.push(format!("[base] seed: {error}"));
        }
    }

    let audio = file.audio.unwrap_or_default();
    let mut bands = Vec::new();
    for (name, [lo, hi]) in &audio.bands {
        if !lo.is_finite() || !hi.is_finite() {
            errors.push(format!(
                "band {name:?}: edges must be finite, got [{lo}, {hi}]"
            ));
        } else if *lo < 0.0 || *hi <= *lo {
            errors.push(format!(
                "band {name:?}: range must satisfy 0 <= lo < hi, got [{lo}, {hi}]"
            ));
        }
        bands.push((name.clone(), [*lo, *hi]));
    }
    let normalize = match audio.normalize.as_deref().map(parse_normalize) {
        None => Normalize::Peak,
        Some(Ok(normalize)) => normalize,
        Some(Err(error)) => {
            errors.push(format!("[audio] {error}"));
            Normalize::Peak
        }
    };
    let onset_sensitivity = audio.onset_sensitivity.unwrap_or(1.5);
    if !onset_sensitivity.is_finite() || onset_sensitivity <= 0.0 {
        errors.push(format!(
            "[audio] onset_sensitivity must be finite and > 0, got {onset_sensitivity}"
        ));
    }

    let mut modulations = Vec::new();
    for (index, modulation) in file.modulate.iter().enumerate() {
        let tag = format!(
            "[[modulate]] #{} (target {:?})",
            index + 1,
            modulation.target
        );
        let param_index = catalog.index_of(&modulation.target);
        if param_index.is_none() {
            let suggestions = catalog.suggest(&modulation.target, 3);
            errors.push(format!(
                "{tag}: unknown target; did you mean {}? (run `reactive-vcr params`)",
                suggestions.join(", ")
            ));
        }

        let source = match parse_source(&modulation.source, &bands) {
            Ok(source) => Some(source),
            Err(error) => {
                errors.push(format!("{tag}: {error}"));
                None
            }
        };
        let curve = match Curve::parse(modulation.curve.as_deref().unwrap_or("linear")) {
            Ok(curve) => Some(curve),
            Err(error) => {
                errors.push(format!("{tag}: {error}"));
                None
            }
        };

        if !modulation.attack_ms.is_finite() || modulation.attack_ms < 0.0 {
            errors.push(format!(
                "{tag}: attack_ms must be finite and >= 0, got {}",
                modulation.attack_ms
            ));
        }
        if !modulation.release_ms.is_finite() || modulation.release_ms < 0.0 {
            errors.push(format!(
                "{tag}: release_ms must be finite and >= 0, got {}",
                modulation.release_ms
            ));
        }
        if let Some(threshold) = modulation.threshold
            && (!threshold.is_finite() || !(0.0..=1.0).contains(&threshold))
        {
            errors.push(format!(
                "{tag}: threshold must be finite and within 0..=1, got {threshold}"
            ));
        }
        if let Some([lo, hi]) = modulation.range
            && (!lo.is_finite() || !hi.is_finite())
        {
            errors.push(format!(
                "{tag}: range values must be finite, got [{lo}, {hi}]"
            ));
        }

        let Some(param_index) = param_index else {
            continue;
        };
        let param = catalog.param(param_index);
        let (min, max) = param.kind.bounds();
        let range = if param.kind.is_bool() {
            if modulation.range.is_some() {
                errors.push(format!("{tag}: bool target takes `threshold`, not `range`"));
            }
            (0.0, 1.0)
        } else {
            if modulation.threshold.is_some() {
                errors.push(format!("{tag}: `threshold` is only for bool targets"));
            }
            match modulation.range {
                None => (min as f32, max as f32),
                Some([lo, hi]) => {
                    for value in [lo, hi] {
                        if value.is_finite() && ((value as f64) < min || (value as f64) > max) {
                            errors.push(format!(
                                "{tag}: range value {value} outside parameter bounds {min}..={max} ({})",
                                param.kind.describe()
                            ));
                        }
                    }
                    (lo, hi)
                }
            }
        };

        if let (Some(source), Some(curve)) = (source, curve) {
            modulations.push(CompiledModulation {
                param: param_index,
                source,
                range,
                threshold: modulation.threshold.unwrap_or(0.5),
                curve,
                attack_ms: modulation.attack_ms,
                release_ms: modulation.release_ms,
                invert: modulation.invert,
            });
        }
    }

    if !errors.is_empty() {
        bail!(
            "mapping has {} problem(s):\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        );
    }
    Ok(CompiledMapping {
        preset_path,
        base,
        bands,
        normalize,
        onset_sensitivity,
        modulations,
    })
}
