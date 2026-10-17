use crate::audio::features::FeatureTimeline;
use crate::curve::lerp;
use crate::effect::{ChainLayout, EffectSettings};
use crate::mapping::CompiledMapping;
use crate::params::{ParamCatalog, get_param, set_param};
use crate::smooth::AttackRelease;

pub struct Modulator<'a> {
    catalog: &'a ParamCatalog,
    mapping: &'a CompiledMapping,
    features: &'a FeatureTimeline,
    smoothers: Vec<AttackRelease>,
    targets: Vec<usize>,
    base_values: Vec<f64>,
    last_values: Vec<f64>,
    offsets: Vec<f64>,
    values: Vec<f64>,
}

impl<'a> Modulator<'a> {
    pub fn new(
        catalog: &'a ParamCatalog,
        mapping: &'a CompiledMapping,
        features: &'a FeatureTimeline,
        fps: f64,
    ) -> Self {
        let smoothers = mapping
            .modulations
            .iter()
            .map(|m| AttackRelease::new(m.attack_ms, m.release_ms, fps))
            .collect();
        let mut targets = Vec::new();
        for modulation in &mapping.modulations {
            if !targets.contains(&modulation.param) {
                targets.push(modulation.param);
            }
        }
        let base_values = targets
            .iter()
            .map(|&index| get_param(&mapping.base, catalog.param(index)))
            .collect::<Vec<_>>();
        let target_count = targets.len();
        Self {
            catalog,
            mapping,
            features,
            smoothers,
            targets,
            last_values: base_values.clone(),
            base_values,
            offsets: vec![0.0; target_count],
            values: vec![0.0; target_count],
        }
    }

    pub fn target_indices(&self) -> Vec<usize> {
        self.targets.clone()
    }

    pub fn last_values(&self) -> &[f64] {
        &self.last_values
    }

    /// Advance to `frame` and return each target's clamped value, in `target_indices` order.
    /// Frames must be requested in increasing order.
    pub fn step(&mut self, frame: usize) -> &[f64] {
        self.offsets.fill(0.0);
        for (index, modulation) in self.mapping.modulations.iter().enumerate() {
            let mut x = self.features.value(modulation.source, frame);
            x = self.smoothers[index].process(x);
            if modulation.invert {
                x = 1.0 - x;
            }
            x = modulation.curve.apply(x);
            let param = self.catalog.param(modulation.param);
            let value = if param.kind.is_bool() {
                if x >= modulation.threshold { 1.0 } else { 0.0 }
            } else {
                lerp(modulation.range, x) as f64
            };
            let target = self
                .targets
                .iter()
                .position(|&target| target == modulation.param)
                .expect("modulation target was registered");
            self.offsets[target] += value - self.base_values[target];
        }
        for (target, &param_index) in self.targets.iter().enumerate() {
            let (min, max) = self.catalog.param(param_index).kind.bounds();
            self.values[target] = (self.base_values[target] + self.offsets[target]).clamp(min, max);
        }
        &self.values
    }

    /// Build the effect for `frame`. Frames must be requested in increasing order.
    pub fn effect_for_frame(&mut self, frame: usize) -> EffectSettings {
        self.step(frame);
        let effect = apply_targets(
            self.catalog,
            &self.mapping.base,
            &self.targets,
            &self.values,
        );
        for (target, &param_index) in self.targets.iter().enumerate() {
            self.last_values[target] = get_param(&effect, self.catalog.param(param_index));
        }
        effect
    }
}

/// `base` with each target parameter set to its value from `step`.
fn apply_targets(
    catalog: &ParamCatalog,
    base: &EffectSettings,
    targets: &[usize],
    values: &[f64],
) -> EffectSettings {
    let mut effect = base.clone();
    for (&param_index, &value) in targets.iter().zip(values) {
        set_param(&mut effect, catalog.param(param_index), value)
            .expect("catalog parameter accepts bounded value");
    }
    effect
}

/// Every frame's modulated values, precomputed for random access during playback.
#[derive(Debug, Clone)]
pub struct Schedule {
    base: EffectSettings,
    chain: ChainLayout,
    targets: Vec<usize>,
    /// Frame-major `frames × targets` values from `Modulator::step`.
    values: Vec<f64>,
    frames: usize,
}

impl Schedule {
    /// Run the modulator over every feature frame, exactly as `render` does.
    pub fn build(
        catalog: &ParamCatalog,
        mapping: &CompiledMapping,
        features: &FeatureTimeline,
        fps: f64,
    ) -> Self {
        let mut modulator = Modulator::new(catalog, mapping, features, fps);
        let mut values = Vec::with_capacity(features.frames * modulator.targets.len());
        for frame in 0..features.frames {
            values.extend_from_slice(modulator.step(frame));
        }
        Self {
            base: mapping.base.clone(),
            chain: mapping.chain,
            targets: modulator.targets,
            values,
            frames: features.frames,
        }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn chain(&self) -> ChainLayout {
        self.chain
    }

    /// The effect for `frame`; frames past the end hold the last frame's values.
    pub fn effect(&self, catalog: &ParamCatalog, frame: usize) -> EffectSettings {
        if self.frames == 0 {
            return self.base.clone();
        }
        let count = self.targets.len();
        let start = frame.min(self.frames - 1) * count;
        apply_targets(
            catalog,
            &self.base,
            &self.targets,
            &self.values[start..start + count],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::features::FeatureTimeline;
    use crate::curve::Curve;
    use crate::effect::ChainLayout;
    use crate::mapping::{CompiledMapping, CompiledModulation, Normalize, Source};
    use crate::params::{ParamCatalog, get_param};

    fn features(bass: Vec<f32>, onset: Vec<f32>) -> FeatureTimeline {
        let n = bass.len();
        FeatureTimeline {
            frames: n,
            rms: vec![0.5; n],
            bands: vec![bass],
            onset,
        }
    }

    fn mapping(mods: Vec<CompiledModulation>) -> CompiledMapping {
        CompiledMapping {
            preset_path: None,
            bender_preset_path: None,
            base: EffectSettings::default(),
            chain: ChainLayout::default(),
            bands: vec![("bass".into(), [20.0, 200.0])],
            normalize: Normalize::Peak,
            onset_sensitivity: 1.5,
            modulations: mods,
        }
    }

    fn m(c: &ParamCatalog, target: &str, source: Source, range: (f32, f32)) -> CompiledModulation {
        CompiledModulation {
            param: c.index_of(target).unwrap(),
            source,
            range,
            threshold: 0.5,
            curve: Curve::Linear,
            attack_ms: 0.0,
            release_ms: 0.0,
            invert: false,
        }
    }

    #[test]
    fn band_drives_float_param_through_range() {
        let c = ParamCatalog::new();
        let map = mapping(vec![m(
            &c,
            "tracking_noise_wave_intensity",
            Source::Band(0),
            (2.0, 30.0),
        )]);
        let f = features(vec![0.0, 0.5, 1.0], vec![0.0; 3]);
        let mut mo = Modulator::new(&c, &map, &f, 30.0);
        let p = c.get("tracking_noise_wave_intensity").unwrap();
        assert_eq!(get_param(&mo.effect_for_frame(0), p), 2.0);
        assert_eq!(get_param(&mo.effect_for_frame(1), p), 16.0);
        assert_eq!(get_param(&mo.effect_for_frame(2), p), 30.0);
        assert_eq!(mo.last_values(), &[30.0]);
    }

    #[test]
    fn band_drives_bender_param() {
        let c = ParamCatalog::new();
        let map = mapping(vec![m(
            &c,
            "bender.sync.v_roll",
            Source::Band(0),
            (0.0, 1.0),
        )]);
        let f = features(vec![0.0, 0.5], vec![0.0; 2]);
        let mut mo = Modulator::new(&c, &map, &f, 30.0);
        assert_eq!(mo.effect_for_frame(0).bender.sync.settings.v_roll, 0.0);
        assert_eq!(mo.effect_for_frame(1).bender.sync.settings.v_roll, 0.5);
    }

    #[test]
    fn multiple_mods_sum_offsets_without_clamping() {
        let c = ParamCatalog::new();
        let p = c.get("snow_intensity").unwrap();
        let base_v = get_param(&EffectSettings::default(), p);
        let map = mapping(vec![
            m(&c, "snow_intensity", Source::Band(0), (base_v as f32, 20.0)),
            m(&c, "snow_intensity", Source::Rms, (base_v as f32, 20.0)),
        ]);
        let f = features(vec![1.0], vec![0.0]);
        let mut mo = Modulator::new(&c, &map, &f, 30.0);
        let value = get_param(&mo.effect_for_frame(0), p);
        assert!((value - 30.0).abs() < 0.01, "got {value}");
    }

    #[test]
    fn multiple_mods_sum_offsets_and_clamp() {
        let c = ParamCatalog::new();
        let p = c.get("snow_intensity").unwrap();
        let base_v = get_param(&EffectSettings::default(), p);
        let map = mapping(vec![
            m(&c, "snow_intensity", Source::Band(0), (base_v as f32, 80.0)),
            m(&c, "snow_intensity", Source::Rms, (base_v as f32, 80.0)),
        ]);
        let f = features(vec![1.0, 0.0], vec![0.0; 2]);
        let mut mo = Modulator::new(&c, &map, &f, 30.0);
        assert_eq!(get_param(&mo.effect_for_frame(0), p), 100.0);
        let v = get_param(&mo.effect_for_frame(1), p);
        assert!((v - 40.0).abs() < 0.01, "got {v}");
    }

    #[test]
    fn bool_threshold_and_onset_release() {
        let c = ParamCatalog::new();
        let mut hs = m(&c, "head_switching", Source::Onset, (0.0, 1.0));
        hs.release_ms = 200.0;
        let map = mapping(vec![hs]);
        let f = features(vec![0.0; 6], vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        let mut mo = Modulator::new(&c, &map, &f, 30.0);
        let p = c.get("head_switching").unwrap();
        assert_eq!(get_param(&mo.effect_for_frame(0), p), 0.0);
        assert_eq!(get_param(&mo.effect_for_frame(1), p), 1.0);
        assert_eq!(
            get_param(&mo.effect_for_frame(2), p),
            1.0,
            "still above 0.5 one frame later at 200ms release"
        );
        for fr in 3..6 {
            mo.effect_for_frame(fr);
        }
        let f2 = features(vec![0.0; 40], vec![0.0; 40]);
        let mut mo2 = Modulator::new(&c, &map, &f2, 30.0);
        assert_eq!(get_param(&mo2.effect_for_frame(0), p), 0.0);
    }

    #[test]
    fn invert_and_enum_rounding() {
        let c = ParamCatalog::new();
        let mut e = m(&c, "vhs_tape_speed", Source::Band(0), (0.0, 3.0));
        e.invert = true;
        let map = mapping(vec![e]);
        let f = features(vec![0.0, 0.4], vec![0.0; 2]);
        let mut mo = Modulator::new(&c, &map, &f, 30.0);
        let p = c.get("vhs_tape_speed").unwrap();
        assert_eq!(get_param(&mo.effect_for_frame(0), p), 3.0);
        assert_eq!(get_param(&mo.effect_for_frame(1), p), 2.0);
    }

    #[test]
    fn schedule_random_access_matches_sequential_render() {
        let c = ParamCatalog::new();
        let mut smoothed = m(&c, "snow_intensity", Source::Band(0), (0.0, 80.0));
        smoothed.attack_ms = 50.0;
        smoothed.release_ms = 300.0;
        let mut onset = m(&c, "head_switching", Source::Onset, (0.0, 1.0));
        onset.release_ms = 100.0;
        let map = mapping(vec![
            smoothed,
            onset,
            m(&c, "vhs_tape_speed", Source::Band(0), (0.0, 3.0)),
            m(&c, "bender.sync.v_roll", Source::Rms, (0.0, 1.0)),
        ]);
        let bass = vec![0.0, 1.0, 0.2, 0.9, 0.0, 0.0, 0.6, 0.1];
        let onsets = vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let f = features(bass, onsets);
        let mut sequential = Modulator::new(&c, &map, &f, 30.0);
        let rendered: Vec<_> = (0..f.frames)
            .map(|frame| sequential.effect_for_frame(frame))
            .collect();

        let schedule = Schedule::build(&c, &map, &f, 30.0);
        assert_eq!(schedule.frames(), f.frames);
        let lookups = [5, 0, 7, 3, 3, 1, 6, 2, 4, 100];
        for frame in lookups {
            let expected = &rendered[frame.min(f.frames - 1)];
            let played = schedule.effect(&c, frame);
            for param in c.iter() {
                assert_eq!(
                    get_param(&played, param),
                    get_param(expected, param),
                    "{} at frame {frame}",
                    param.name
                );
            }
        }
    }
}
