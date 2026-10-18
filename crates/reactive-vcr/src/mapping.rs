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
        let file = MappingFile::parse(s)?;
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
        let mut e = EffectSettings::default();
        crate::params::set_param(&mut e, c.get("snow_intensity").unwrap(), 42.0).unwrap();
        std::fs::write(
            dir.path().join("p.json"),
            c.preset_to_json(&e.ntsc).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("m.toml"), "[base]\npreset = \"p.json\"\n").unwrap();
        let m = load(&dir.path().join("m.toml"), &c).unwrap();
        assert_eq!(get_param(&m.base, c.get("snow_intensity").unwrap()), 42.0);
    }

    #[test]
    fn compiles_bender_section() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("b.json"),
            r#"{"version":2,"seed":9,"order":["burst"],"sync":{"enabled":true,"v_roll":0.2}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("m.toml"),
            r#"
[bender]
preset = "b.json"
seed = 4
order = "after"

[bender.set]
order = ["hold", "sync"]
"burst.enabled" = true
burst.phase = 45

[bender.set.bleed]
enabled = true
"#,
        )
        .unwrap();
        let m = load(&dir.path().join("m.toml"), &ParamCatalog::new()).unwrap();
        let b = &m.base.bender;
        assert_eq!(b.sync.settings.v_roll, 0.2);
        assert!(b.burst.enabled);
        assert_eq!(b.burst.settings.phase, 45.0);
        assert!(b.bleed.enabled);
        assert_eq!(b.seed, 4);
        assert_eq!(
            b.order.stages()[..3],
            [
                bender::StageId::Hold,
                bender::StageId::Sync,
                bender::StageId::Sensor
            ],
            "set order replaces the preset order; unlisted stages keep default order"
        );
        assert_eq!(m.chain.bend_order, BendOrder::After);
        assert!(m.chain.ntsc);
        assert!(
            m.preset_inputs()
                .iter()
                .any(|(label, _)| *label == "bender preset")
        );
    }

    #[test]
    fn reports_bender_set_errors_together() {
        let err = compile_str(
            r#"
[bender.set]
"sync.v_rol" = 0.1
"sync.v_roll" = 3
"invert.mode" = "sideways"
"rail.sag" = [1]
seed = 1
order = ["sync", "warp"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("\"sync.v_rol\"; did you mean sync.v_roll"),
            "{err}"
        );
        for expected in [
            "out of range",
            "sideways",
            "array",
            "[bender] seed",
            "stage \"warp\"; stages: sensor, retina",
        ] {
            assert!(err.contains(expected), "missing {expected:?} from {err}");
        }
    }

    #[test]
    fn stage_order_accepts_comma_list_and_rejects_repeats() {
        let m = compile_str("[bender.set]\norder = \"feedback, dct\"\n").unwrap();
        assert_eq!(
            m.base.bender.order.stages()[..2],
            [bender::StageId::Feedback, bender::StageId::Dct]
        );
        let err = compile_str("[bender.set]\norder = \"dct,dct\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("repeated stage \"dct\""), "{err}");
        let err = compile_str("[bender.set]\norder = 3\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("expected an array of stage names, got integer"),
            "{err}"
        );
    }

    #[test]
    fn ntsc_bypass_rejects_ntsc_only_options() {
        let err = compile_str(
            r#"
[base]
ntsc = false
seed = 1

[[modulate]]
target = "snow_intensity"
source = "rms"

[[modulate]]
target = "bender.sync.v_roll"
source = "rms"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("[base] seed"), "{err}");
        assert!(err.contains("ntsc-rs target"), "{err}");
        let m = compile_str(
            r#"
[base]
ntsc = false

[[modulate]]
target = "bender.sync.v_roll"
source = "rms"
"#,
        )
        .unwrap();
        assert!(!m.chain.ntsc);
    }

    #[test]
    fn rejects_unknown_bend_order() {
        let err = compile_str("[bender]\norder = \"sideways\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("before"), "{err}");
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

    #[test]
    fn base_set_overrides_ntsc_settings_by_number_bool_and_label() {
        let m = compile_str(
            r#"
[base.set]
snow_intensity = 2.5
head_switching = true
vhs_tape_speed = "lp (long play)"
tracking_noise_height = 12
"#,
        )
        .unwrap();
        let c = ParamCatalog::new();
        let value = |name| get_param(&m.base, c.get(name).unwrap());
        assert_eq!(value("snow_intensity"), 2.5);
        assert_eq!(value("head_switching"), 1.0);
        assert_eq!(value("tracking_noise_height"), 12.0);
        let ParamKind::Enum { options } = &c.get("vhs_tape_speed").unwrap().kind else {
            panic!("vhs_tape_speed is an enum");
        };
        let lp = options
            .iter()
            .position(|(_, label)| label.eq_ignore_ascii_case("lp (long play)"))
            .unwrap();
        assert_eq!(value("vhs_tape_speed"), lp as f64);
    }

    #[test]
    fn base_set_rejects_unknown_bender_and_mistyped_entries() {
        let err = compile_str(
            r#"
[base.set]
snow_intensty = 1.0
"bender.sync.v_roll" = 0.2
head_switching = 3
random_seed = 4
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("4 problem"), "{err}");
        assert!(err.contains("snow_intensity"), "suggestion missing: {err}");
        assert!(err.contains("\"bender.sync.v_roll\""), "{err}");
        assert!(
            err.contains("head_switching: expected true or false"),
            "{err}"
        );
        assert!(err.contains("use [base] seed"), "{err}");
    }

    #[test]
    fn edited_mapping_round_trips_through_toml() {
        let c = ParamCatalog::new();
        let mut file = MappingFile::parse(GOOD).unwrap();
        file.set_base_value(c.get("snow_intensity").unwrap(), 3.0);
        file.set_base_value(c.get("bender.invert.mode").unwrap(), 2.0);
        file.set_base_value(c.get("bender.invert.enabled").unwrap(), 1.0);
        let text = file.to_toml().unwrap();
        let reparsed = MappingFile::parse(&text).unwrap();
        assert_eq!(reparsed, file, "{text}");

        let compiled = compile(reparsed, Path::new("."), &c).unwrap();
        let value = |name| get_param(&compiled.base, c.get(name).unwrap());
        assert_eq!(value("snow_intensity"), 3.0);
        assert_eq!(value("bender.invert.mode"), 2.0);
        assert_eq!(value("bender.invert.enabled"), 1.0);
        assert_eq!(value("random_seed"), 7.0);
        assert_eq!(compiled.modulations.len(), 3);

        file.clear_base_value(c.get("snow_intensity").unwrap());
        let cleared = compile(file, Path::new("."), &c).unwrap();
        assert_eq!(
            get_param(&cleared.base, c.get("snow_intensity").unwrap()),
            get_param(&EffectSettings::default(), c.get("snow_intensity").unwrap())
        );
    }

    #[test]
    fn nested_set_tables_flatten_to_one_entry_per_setting() {
        let file = MappingFile::parse(
            r#"
[bender.set.sync]
enabled = true
v_roll = 0.25
"#,
        )
        .unwrap();
        let set = &file.bender.unwrap().set;
        assert_eq!(set.get("sync.v_roll"), Some(&toml::Value::Float(0.25)));
        assert_eq!(set.len(), 2);
    }
}
use crate::curve::Curve;
use crate::effect::{BendOrder, ChainLayout, EffectSettings};
use crate::params::{BENDER_PREFIX, Param, ParamCatalog, ParamKind, ParamTarget, set_param};
use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MappingFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BaseSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bender: Option<BenderSection>,
    #[serde(default)]
    pub modulate: Vec<ModulationSection>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BaseSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,
    /// Run the ntsc-rs stage (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ntsc: Option<bool>,
    /// ntsc-rs setting names to values, applied over the preset.
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub set: toml::Table,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BenderSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<BendOrder>,
    /// bender-rs setting ids to values; quoted, dotted, and nested keys are equivalent.
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub set: toml::Table,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AudioSection {
    #[serde(default)]
    pub bands: BTreeMap<String, [f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalize: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onset_sensitivity: Option<f32>,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModulationSection {
    pub target: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attack_ms: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub release_ms: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
}

impl MappingFile {
    pub fn parse(text: &str) -> Result<Self> {
        let mut file: Self = toml::from_str(text)?;
        file.flatten_sets();
        Ok(file)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).context("serializing mapping")
    }

    /// Rewrites nested `set` tables as dotted keys, so each setting has exactly one entry.
    fn flatten_sets(&mut self) {
        let sets = [
            self.base.as_mut().map(|base| &mut base.set),
            self.bender.as_mut().map(|bender| &mut bender.set),
        ];
        for set in sets.into_iter().flatten() {
            let mut flat = Vec::new();
            flatten_table("", set, &mut flat);
            *set = flat.into_iter().collect();
        }
    }

    /// Sets a base (unmodulated) value in `[base.set]` or `[bender.set]`.
    pub fn set_base_value(&mut self, param: &Param, value: f64) {
        let (table, key, value) = match &param.target {
            ParamTarget::Ntsc(_) => (
                &mut self.base.get_or_insert_with(Default::default).set,
                param.name.to_string(),
                toml_number(&param.kind, value),
            ),
            ParamTarget::Bender { .. } => {
                let id = param.name.strip_prefix(BENDER_PREFIX).unwrap_or(param.name);
                let value = match &param.kind {
                    ParamKind::Enum { .. } => bender::find_setting(id)
                        .and_then(|desc| match desc.kind {
                            bender::SettingKind::Enum { options } => options
                                .get(value.round().max(0.0) as usize)
                                .map(|(name, _)| toml::Value::String(name.to_string())),
                            _ => None,
                        })
                        .unwrap_or_else(|| toml_number(&param.kind, value)),
                    kind => toml_number(kind, value),
                };
                (
                    &mut self.bender.get_or_insert_with(Default::default).set,
                    id.to_string(),
                    value,
                )
            }
        };
        table.insert(key, value);
    }

    /// Removes a base value, restoring the preset or default.
    pub fn clear_base_value(&mut self, param: &Param) {
        let (table, key) = match &param.target {
            ParamTarget::Ntsc(_) => (self.base.as_mut().map(|base| &mut base.set), param.name),
            ParamTarget::Bender { .. } => (
                self.bender.as_mut().map(|bender| &mut bender.set),
                param.name.strip_prefix(BENDER_PREFIX).unwrap_or(param.name),
            ),
        };
        if let Some(table) = table {
            table.remove(key);
        }
    }
}

fn toml_number(kind: &ParamKind, value: f64) -> toml::Value {
    match kind {
        ParamKind::Float { .. } => toml::Value::Float(value),
        ParamKind::Int { .. } | ParamKind::Enum { .. } => {
            toml::Value::Integer(value.round() as i64)
        }
        ParamKind::Bool => toml::Value::Boolean(value >= 0.5),
    }
}

/// The catalog value for a `[base.set]` entry: a number, a boolean, or an enum label.
fn base_set_value(param: &Param, value: &toml::Value) -> Result<f64> {
    match (&param.kind, value) {
        (ParamKind::Bool, toml::Value::Boolean(b)) => Ok(f64::from(*b)),
        (ParamKind::Bool, other) => bail!("expected true or false, got {}", other.type_str()),
        (_, toml::Value::Integer(i)) => Ok(*i as f64),
        (ParamKind::Float { .. }, toml::Value::Float(f)) => Ok(*f),
        (ParamKind::Enum { options }, toml::Value::String(label)) => options
            .iter()
            .position(|(_, option)| option.eq_ignore_ascii_case(label))
            .map(|index| index as f64)
            .ok_or_else(|| anyhow!("unknown option {label:?}; {}", param.kind.describe())),
        (_, other) => bail!(
            "expected a value for {}, got {}",
            param.kind.describe(),
            other.type_str()
        ),
    }
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
    /// bender-rs preset input resolved relative to the mapping, retained for output protection.
    pub bender_preset_path: Option<PathBuf>,
    pub base: EffectSettings,
    pub chain: ChainLayout,
    pub bands: Vec<(String, [f32; 2])>,
    pub normalize: Normalize,
    pub onset_sensitivity: f32,
    pub modulations: Vec<CompiledModulation>,
}

impl CompiledMapping {
    /// Preset files read by this mapping, labeled for error messages.
    pub fn preset_inputs(&self) -> Vec<(&'static str, &Path)> {
        [
            ("preset", self.preset_path.as_deref()),
            ("bender preset", self.bender_preset_path.as_deref()),
        ]
        .into_iter()
        .filter_map(|(label, path)| path.map(|path| (label, path)))
        .collect()
    }
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

/// Flattens nested tables into dotted keys so `"a.b" = 1`, `a.b = 1`, and `[x.a] b = 1` agree.
fn flatten_table(prefix: &str, table: &toml::Table, out: &mut Vec<(String, toml::Value)>) {
    for (key, value) in table {
        let id = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) => flatten_table(&id, inner, out),
            other => out.push((id, other.clone())),
        }
    }
}

/// Applies `[bender.set] order`: stage names as an array or a comma-separated string.
/// Listed stages run first; the rest follow in bender-rs's default order.
fn set_stage_order(effect: &mut bender::BendEffect, value: &toml::Value, errors: &mut Vec<String>) {
    let raw = match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Array(items) => items
            .iter()
            .map(toml::Value::as_str)
            .collect::<Option<Vec<_>>>()
            .map(|names| names.join(",")),
        _ => None,
    };
    let Some(raw) = raw else {
        errors.push(format!(
            "[bender.set] order: expected an array of stage names, got {}",
            value.type_str()
        ));
        return;
    };
    match bender::ChainOrder::parse(&raw) {
        Ok(order) => effect.order = order,
        Err(name) => errors.push(format!(
            "[bender.set] order: unknown or repeated stage {name:?}; stages: {}",
            bender::StageId::ALL.map(bender::StageId::name).join(", ")
        )),
    }
}

pub fn load(path: &Path, catalog: &ParamCatalog) -> Result<CompiledMapping> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading mapping {}", path.display()))?;
    let file =
        MappingFile::parse(&text).with_context(|| format!("parsing mapping {}", path.display()))?;
    compile(file, path.parent().unwrap_or(Path::new(".")), catalog)
}

pub fn compile(
    file: MappingFile,
    base_dir: &Path,
    catalog: &ParamCatalog,
) -> Result<CompiledMapping> {
    compile_with(file, base_dir, catalog, |path| {
        std::fs::read_to_string(path).map_err(Into::into)
    })
}

/// [`compile`] with preset files supplied by `read` instead of the filesystem.
pub fn compile_with(
    file: MappingFile,
    base_dir: &Path,
    catalog: &ParamCatalog,
    mut read: impl FnMut(&Path) -> Result<String>,
) -> Result<CompiledMapping> {
    let mut errors = Vec::new();
    let base_section = file.base.unwrap_or_default();
    let ntsc_enabled = base_section.ntsc.unwrap_or(true);
    if !ntsc_enabled {
        if base_section.preset.is_some() {
            errors.push("[base] preset has no effect while [base] ntsc = false".to_string());
        }
        if base_section.seed.is_some() {
            errors.push("[base] seed has no effect while [base] ntsc = false".to_string());
        }
        if !base_section.set.is_empty() {
            errors.push("[base.set] has no effect while [base] ntsc = false".to_string());
        }
    }
    let mut base = EffectSettings::default();
    let preset_path = base_section.preset.map(|preset| base_dir.join(preset));
    if let Some(full) = &preset_path {
        match read(full)
            .with_context(|| format!("reading preset {}", full.display()))
            .and_then(|json| {
                catalog
                    .preset_from_json(&json)
                    .with_context(|| format!("preset {}", full.display()))
            }) {
            Ok(effect) => base.ntsc = effect,
            Err(error) => errors.push(error.to_string()),
        }
    }
    let mut ntsc_settings = Vec::new();
    flatten_table("", &base_section.set, &mut ntsc_settings);
    for (name, value) in ntsc_settings {
        if name == "random_seed" {
            errors.push("[base.set] random_seed: use [base] seed".to_string());
            continue;
        }
        let Some(param) = catalog
            .get(&name)
            .filter(|param| matches!(param.target, ParamTarget::Ntsc(_)))
        else {
            let suggestions = catalog
                .suggest(&name, 3)
                .into_iter()
                .filter(|suggestion| !suggestion.starts_with(BENDER_PREFIX))
                .collect::<Vec<_>>()
                .join(", ");
            errors.push(format!(
                "[base.set] unknown ntsc-rs setting {name:?}; did you mean {suggestions}? (run `reactive-vcr params`)"
            ));
            continue;
        };
        match base_set_value(param, &value) {
            Ok(number) => {
                if let Err(error) = set_param(&mut base, param, number) {
                    errors.push(format!("[base.set] {error:#}"));
                }
            }
            Err(error) => errors.push(format!("[base.set] {name}: {error}")),
        }
    }
    if let Some(seed) = base_section.seed {
        let param = catalog.get("random_seed").expect("ntsc-rs has random_seed");
        if let Err(error) = set_param(&mut base, param, seed as f64) {
            errors.push(format!("[base] seed: {error}"));
        }
    }

    let bender_section = file.bender.unwrap_or_default();
    let bender_preset_path = bender_section.preset.map(|preset| base_dir.join(preset));
    if let Some(full) = &bender_preset_path {
        match read(full)
            .with_context(|| format!("reading bender preset {}", full.display()))
            .and_then(|json| {
                bender::BendEffect::from_json(&json)
                    .map_err(|error| anyhow!("bender preset {}: {error}", full.display()))
            }) {
            Ok(effect) => base.bender = effect,
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    let mut bender_settings = Vec::new();
    flatten_table("", &bender_section.set, &mut bender_settings);
    for (id, value) in bender_settings {
        if id == "seed" {
            errors.push("[bender.set] seed: use [bender] seed".to_string());
            continue;
        }
        if id == "order" {
            set_stage_order(&mut base.bender, &value, &mut errors);
            continue;
        }
        let raw = match value {
            toml::Value::String(s) => s,
            toml::Value::Integer(i) => i.to_string(),
            toml::Value::Float(f) => f.to_string(),
            toml::Value::Boolean(b) => b.to_string(),
            other => {
                errors.push(format!(
                    "[bender.set] {id}: expected a number, boolean, or enum name, got {}",
                    other.type_str()
                ));
                continue;
            }
        };
        match base.bender.set_by_id(&id, &raw) {
            Ok(()) => {}
            Err(bender::PresetError::UnknownSetting(_)) => {
                let suggestions = catalog
                    .suggest(&format!("{BENDER_PREFIX}{id}"), 3)
                    .into_iter()
                    .map(|name| name.strip_prefix(BENDER_PREFIX).unwrap_or(name))
                    .collect::<Vec<_>>()
                    .join(", ");
                errors.push(format!(
                    "[bender.set] unknown setting {id:?}; did you mean {suggestions}? (run `reactive-vcr params`)"
                ));
            }
            Err(error) => errors.push(format!("[bender.set] {error}")),
        }
    }
    if let Some(seed) = bender_section.seed {
        base.bender.seed = seed;
    }
    let chain = ChainLayout {
        ntsc: ntsc_enabled,
        bend_order: bender_section.order.unwrap_or_default(),
    };

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
        if !ntsc_enabled && matches!(param.target, ParamTarget::Ntsc(_)) {
            errors.push(format!(
                "{tag}: ntsc-rs target has no effect while [base] ntsc = false"
            ));
        }
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
        bender_preset_path,
        base,
        chain,
        bands,
        normalize,
        onset_sensitivity,
        modulations,
    })
}
