#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_known_params_with_kinds() {
        let c = ParamCatalog::new();
        assert!(c.len() >= 60, "expected ~62 params, got {}", c.len());
        match &c.get("snow_intensity").unwrap().kind {
            ParamKind::Float { min, max, .. } => {
                assert_eq!(*min, 0.0);
                assert_eq!(*max, 100.0);
            }
            k => panic!("wrong kind {k:?}"),
        }
        assert!(matches!(
            c.get("head_switching").unwrap().kind,
            ParamKind::Bool
        ));
        assert!(c.get("head_switching").unwrap().is_group);
        match &c.get("vhs_tape_speed").unwrap().kind {
            ParamKind::Enum { options } => assert_eq!(options.len(), 4),
            k => panic!("wrong kind {k:?}"),
        }
        assert!(matches!(
            c.get("head_switching_height").unwrap().kind,
            ParamKind::Int { min: 0, max: 24 }
        ));
        assert!(c.get("nope").is_none());
    }

    #[test]
    fn catalog_includes_prefixed_bender_settings() {
        let c = ParamCatalog::new();
        assert!(matches!(
            c.get("bender.sync.v_roll").unwrap().kind,
            ParamKind::Float {
                min: -1.0,
                max: 1.0,
                ..
            }
        ));
        let enabled = c.get("bender.sync.enabled").unwrap();
        assert!(matches!(enabled.kind, ParamKind::Bool));
        assert!(enabled.is_group);
        assert!(matches!(
            c.get("bender.clock.sample_hold").unwrap().kind,
            ParamKind::Int { min: 1, max: 64 }
        ));
        match &c.get("bender.invert.mode").unwrap().kind {
            ParamKind::Enum { options } => assert_eq!(options.len(), 4),
            k => panic!("wrong kind {k:?}"),
        }
        let count = c
            .iter()
            .filter(|p| p.name.starts_with(BENDER_PREFIX))
            .count();
        assert_eq!(count, 57);
    }

    #[test]
    fn bender_set_and_get_roundtrip_with_clamping() {
        let c = ParamCatalog::new();
        let mut e = EffectSettings::default();
        let v_roll = c.get("bender.sync.v_roll").unwrap();
        set_param(&mut e, v_roll, 5.0).unwrap();
        assert_eq!(get_param(&e, v_roll), 1.0);
        assert_eq!(e.bender.sync.settings.v_roll, 1.0);
        let hold = c.get("bender.clock.sample_hold").unwrap();
        set_param(&mut e, hold, 7.6).unwrap();
        assert_eq!(get_param(&e, hold), 8.0);
        assert_eq!(e.bender.clock.settings.sample_hold, 8);
        let enabled = c.get("bender.sync.enabled").unwrap();
        set_param(&mut e, enabled, 0.7).unwrap();
        assert_eq!(get_param(&e, enabled), 1.0);
        assert!(e.bender.sync.enabled);
        let mode = c.get("bender.invert.mode").unwrap();
        set_param(&mut e, mode, 9.0).unwrap();
        assert_eq!(get_param(&e, mode), 3.0);
    }

    #[test]
    fn suggest_finds_near_names() {
        let c = ParamCatalog::new();
        let s = c.suggest("snow_intensty", 3);
        assert_eq!(s[0], "snow_intensity");
    }

    #[test]
    fn set_and_get_roundtrip_with_clamping() {
        let c = ParamCatalog::new();
        let mut e = EffectSettings::default();
        set_param(&mut e, c.get("snow_intensity").unwrap(), 250.0).unwrap();
        assert_eq!(get_param(&e, c.get("snow_intensity").unwrap()), 100.0);
        set_param(&mut e, c.get("head_switching_height").unwrap(), 7.6).unwrap();
        assert_eq!(get_param(&e, c.get("head_switching_height").unwrap()), 8.0);
        set_param(&mut e, c.get("head_switching").unwrap(), 0.2).unwrap();
        assert_eq!(get_param(&e, c.get("head_switching").unwrap()), 0.0);
        set_param(&mut e, c.get("head_switching").unwrap(), 0.7).unwrap();
        assert_eq!(get_param(&e, c.get("head_switching").unwrap()), 1.0);
        set_param(&mut e, c.get("vhs_tape_speed").unwrap(), 2.0).unwrap();
        assert_eq!(get_param(&e, c.get("vhs_tape_speed").unwrap()), 2.0);
        set_param(&mut e, c.get("vhs_tape_speed").unwrap(), 9.0).unwrap();
        assert_eq!(get_param(&e, c.get("vhs_tape_speed").unwrap()), 3.0);
    }

    #[test]
    fn preset_json_roundtrip() {
        let c = ParamCatalog::new();
        let mut e = EffectSettings::default();
        set_param(&mut e, c.get("snow_intensity").unwrap(), 3.5).unwrap();
        let json = c.preset_to_json(&e.ntsc).unwrap();
        assert!(json.contains("\"snow_intensity\""));
        let back = EffectSettings {
            ntsc: c.preset_from_json(&json).unwrap(),
            ..EffectSettings::default()
        };
        assert_eq!(get_param(&back, c.get("snow_intensity").unwrap()), 3.5);
        assert!(c.preset_from_json("{not json").is_err());
    }

    #[test]
    fn set_param_rejects_nonfinite_values() {
        let c = ParamCatalog::new();
        let mut e = EffectSettings::default();
        assert!(set_param(&mut e, c.get("snow_intensity").unwrap(), f64::NAN).is_err());
        assert!(set_param(&mut e, c.get("snow_intensity").unwrap(), f64::INFINITY).is_err());
        assert!(
            set_param(
                &mut e,
                c.get("head_switching_height").unwrap(),
                f64::NEG_INFINITY
            )
            .is_err()
        );
    }

    #[test]
    fn kind_descriptions_are_concise_and_stable() {
        assert_eq!(
            ParamKind::Float {
                min: 0.0,
                max: 100.0,
                logarithmic: true
            }
            .describe(),
            "float 0..=100 (log)"
        );
        assert_eq!(ParamKind::Int { min: 0, max: 24 }.describe(), "int 0..=24");
        assert_eq!(ParamKind::Bool.describe(), "bool");
        assert_eq!(
            ParamKind::Enum {
                options: vec![(2, "SP".to_string()), (5, "LP".to_string())]
            }
            .describe(),
            "enum [0=SP, 1=LP]"
        );
    }
}
use crate::effect::EffectSettings;
use anyhow::{Context as _, Result, anyhow};
use ntsc_rs::NtscEffect;
use ntsc_rs::settings::{
    EnumValue, SettingDescriptor, SettingID, SettingKind, Settings, SettingsList,
};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Catalog name prefix for bender-rs settings, e.g. `bender.sync.v_roll`.
pub const BENDER_PREFIX: &str = "bender.";

#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    Float {
        min: f32,
        max: f32,
        logarithmic: bool,
    },
    Int {
        min: i32,
        max: i32,
    },
    Bool,
    Enum {
        options: Vec<(u32, String)>,
    },
}

impl ParamKind {
    pub fn bounds(&self) -> (f64, f64) {
        match self {
            Self::Float { min, max, .. } => (*min as f64, *max as f64),
            Self::Int { min, max } => (*min as f64, *max as f64),
            Self::Bool => (0.0, 1.0),
            Self::Enum { options } => (0.0, options.len().saturating_sub(1) as f64),
        }
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool)
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Float {
                min,
                max,
                logarithmic,
            } => format!(
                "float {min}..={max}{}",
                if *logarithmic { " (log)" } else { "" }
            ),
            Self::Int { min, max } => format!("int {min}..={max}"),
            Self::Bool => "bool".to_string(),
            Self::Enum { options } => format!(
                "enum [{}]",
                options
                    .iter()
                    .enumerate()
                    .map(|(i, (_, label))| format!("{i}={label}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: &'static str,
    pub label: &'static str,
    pub is_group: bool,
    pub kind: ParamKind,
    pub target: ParamTarget,
}

/// The engine setting a catalog entry reads and writes.
#[derive(Debug, Clone)]
pub enum ParamTarget {
    Ntsc(SettingID<NtscEffect>),
    Bender {
        get: fn(&bender::BendEffect) -> bender::SettingValue,
        set: fn(&mut bender::BendEffect, bender::SettingValue),
    },
}

struct BenderEntry {
    name: String,
    label: String,
    is_group: bool,
    kind: ParamKind,
    get: fn(&bender::BendEffect) -> bender::SettingValue,
    set: fn(&mut bender::BendEffect, bender::SettingValue),
}

static BENDER_ENTRIES: LazyLock<Vec<BenderEntry>> = LazyLock::new(|| {
    let mut entries = Vec::new();
    for group in bender::setting_groups() {
        for (i, desc) in group.settings.iter().enumerate() {
            entries.push(BenderEntry {
                name: format!("{BENDER_PREFIX}{}", desc.id),
                label: if i == 0 {
                    group.label.to_string()
                } else {
                    format!("{}: {}", group.label, desc.label)
                },
                is_group: i == 0,
                kind: bender_kind_of(&desc.kind),
                get: desc.get,
                set: desc.set,
            });
        }
    }
    entries
});

fn bender_kind_of(kind: &bender::SettingKind) -> ParamKind {
    match *kind {
        bender::SettingKind::Float {
            min,
            max,
            logarithmic,
        } => ParamKind::Float {
            min,
            max,
            logarithmic,
        },
        bender::SettingKind::Int { min, max } => ParamKind::Int { min, max },
        bender::SettingKind::Bool => ParamKind::Bool,
        bender::SettingKind::Enum { options } => ParamKind::Enum {
            options: options
                .iter()
                .enumerate()
                .map(|(i, (_, label))| (i as u32, label.to_string()))
                .collect(),
        },
    }
}

fn kind_of(desc: &SettingDescriptor<NtscEffect>) -> ParamKind {
    match &desc.kind {
        SettingKind::Enumeration { options } => ParamKind::Enum {
            options: options
                .iter()
                .map(|option| (option.index, option.label.to_string()))
                .collect(),
        },
        SettingKind::Percentage { logarithmic } => ParamKind::Float {
            min: 0.0,
            max: 1.0,
            logarithmic: *logarithmic,
        },
        SettingKind::IntRange { range } => ParamKind::Int {
            min: *range.start(),
            max: *range.end(),
        },
        SettingKind::FloatRange { range, logarithmic } => ParamKind::Float {
            min: *range.start(),
            max: *range.end(),
            logarithmic: *logarithmic,
        },
        SettingKind::Boolean | SettingKind::Group { .. } => ParamKind::Bool,
    }
}

pub struct ParamCatalog {
    list: SettingsList<NtscEffect>,
    params: Vec<Param>,
    by_name: HashMap<&'static str, usize>,
}

impl Default for ParamCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamCatalog {
    pub fn new() -> Self {
        let list = SettingsList::<NtscEffect>::new();
        let mut params = Vec::new();
        let mut by_name = HashMap::new();
        for desc in list.all_descriptors() {
            by_name.insert(desc.id.name, params.len());
            params.push(Param {
                name: desc.id.name,
                label: desc.label,
                is_group: matches!(desc.kind, SettingKind::Group { .. }),
                kind: kind_of(desc),
                target: ParamTarget::Ntsc(desc.id.clone()),
            });
        }
        for entry in LazyLock::force(&BENDER_ENTRIES) {
            by_name.insert(entry.name.as_str(), params.len());
            params.push(Param {
                name: entry.name.as_str(),
                label: entry.label.as_str(),
                is_group: entry.is_group,
                kind: entry.kind.clone(),
                target: ParamTarget::Bender {
                    get: entry.get,
                    set: entry.set,
                },
            });
        }
        Self {
            list,
            params,
            by_name,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Param> {
        self.index_of(name).map(|index| &self.params[index])
    }
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }
    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.params.iter()
    }
    pub fn len(&self) -> usize {
        self.params.len()
    }
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
    pub fn param(&self, i: usize) -> &Param {
        &self.params[i]
    }

    pub fn suggest(&self, name: &str, n: usize) -> Vec<&'static str> {
        let mut scored: Vec<_> = self
            .params
            .iter()
            .map(|param| (strsim::levenshtein(name, param.name), param.name))
            .collect();
        scored.sort();
        scored.into_iter().take(n).map(|(_, name)| name).collect()
    }

    pub fn preset_from_json(&self, json: &str) -> Result<NtscEffect> {
        self.list
            .from_json(json)
            .map_err(|error| anyhow!("invalid ntsc-rs preset: {error}"))
    }

    pub fn preset_to_json(&self, effect: &NtscEffect) -> Result<String> {
        self.list
            .to_json_string(effect)
            .map_err(|error| anyhow!("cannot serialize preset: {error}"))
    }
}

pub fn set_param(effects: &mut EffectSettings, param: &Param, value: f64) -> Result<()> {
    if !value.is_finite() {
        return Err(anyhow!("{}: value must be finite", param.name));
    }
    let id = match &param.target {
        ParamTarget::Ntsc(id) => id,
        ParamTarget::Bender { set, .. } => {
            let v = match &param.kind {
                ParamKind::Float { min, max, .. } => {
                    bender::SettingValue::Float((value as f32).clamp(*min, *max))
                }
                ParamKind::Int { min, max } => {
                    bender::SettingValue::Int(value.round().clamp(*min as f64, *max as f64) as i32)
                }
                ParamKind::Bool => bender::SettingValue::Bool(value >= 0.5),
                ParamKind::Enum { options } => bender::SettingValue::Enum(
                    value
                        .round()
                        .clamp(0.0, options.len().saturating_sub(1) as f64)
                        as u32,
                ),
            };
            set(&mut effects.bender, v);
            return Ok(());
        }
    };
    let effect = &mut effects.ntsc;
    let result = match &param.kind {
        ParamKind::Float { min, max, .. } => {
            effect.set_field::<f32>(id, (value as f32).clamp(*min, *max))
        }
        ParamKind::Int { min, max } => {
            effect.set_field::<i32>(id, value.round().clamp(*min as f64, *max as f64) as i32)
        }
        ParamKind::Bool => effect.set_field::<bool>(id, value >= 0.5),
        ParamKind::Enum { options } => {
            let index = value
                .round()
                .clamp(0.0, options.len().saturating_sub(1) as f64)
                as usize;
            effect.set_field::<EnumValue>(id, EnumValue(options[index].0))
        }
    };
    result
        .map_err(|error| anyhow!("{}: {error}", param.name))
        .context("set_param")
}

pub fn get_param(effects: &EffectSettings, param: &Param) -> f64 {
    let id = match &param.target {
        ParamTarget::Ntsc(id) => id,
        ParamTarget::Bender { get, .. } => {
            return match get(&effects.bender) {
                bender::SettingValue::Float(v) => v as f64,
                bender::SettingValue::Int(v) => v as f64,
                bender::SettingValue::Bool(b) => f64::from(b),
                bender::SettingValue::Enum(i) => i as f64,
            };
        }
    };
    let effect = &effects.ntsc;
    match &param.kind {
        ParamKind::Float { .. } => effect.get_field::<f32>(id).unwrap_or(0.0) as f64,
        ParamKind::Int { .. } => effect.get_field::<i32>(id).unwrap_or(0) as f64,
        ParamKind::Bool => f64::from(effect.get_field::<bool>(id).unwrap_or(false)),
        ParamKind::Enum { options } => {
            let value = effect
                .get_field::<EnumValue>(id)
                .map(|value| value.0)
                .unwrap_or(0);
            options
                .iter()
                .position(|(index, _)| *index == value)
                .unwrap_or(0) as f64
        }
    }
}
