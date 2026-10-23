use crate::audio::features::FeatureTimeline;
use crate::curve::lerp;
use crate::effect::{ChainLayout, EffectSettings};
use crate::mapping::{CompiledMapping, CompiledModulation, Source};
use crate::params::{ParamCatalog, get_param, set_param};
use crate::smooth::AttackRelease;

/// Stateful per-frame modulation: smoothing, curves, and summed offsets per target.
pub struct Modulator {
    modulations: Vec<CompiledModulation>,
    /// Per modulation: whether its target parameter is boolean.
    is_bool: Vec<bool>,
    /// Per modulation: index of its target in `targets`.
    slots: Vec<usize>,
    smoothers: Vec<AttackRelease>,
    targets: Vec<usize>,
    /// Per target: the parameter's `(min, max)`.
    bounds: Vec<(f64, f64)>,
    base_values: Vec<f64>,
    last_values: Vec<f64>,
    offsets: Vec<f64>,
    values: Vec<f64>,
}

impl Modulator {
    pub fn new(catalog: &ParamCatalog, mapping: &CompiledMapping, fps: f64) -> Self {
        let modulations = mapping.modulations.clone();
        let smoothers = modulations
            .iter()
            .map(|m| AttackRelease::new(m.attack_ms, m.release_ms, fps))
            .collect();
        let mut targets = Vec::new();
        for modulation in &modulations {
            if !targets.contains(&modulation.param) {
                targets.push(modulation.param);
            }
        }
        let slots = modulations
            .iter()
            .map(|m| {
                targets
                    .iter()
                    .position(|&target| target == m.param)
                    .expect("modulation target was registered")
            })
            .collect();
        let is_bool = modulations
            .iter()
            .map(|m| catalog.param(m.param).kind.is_bool())
            .collect();
        let bounds = targets
            .iter()
            .map(|&index| catalog.param(index).kind.bounds())
            .collect();
        let base_values = targets
            .iter()
            .map(|&index| get_param(&mapping.base, catalog.param(index)))
            .collect::<Vec<_>>();
        let target_count = targets.len();
        Self {
            modulations,
            is_bool,
            slots,
            smoothers,
            targets,
            bounds,
            last_values: base_values.clone(),
            base_values,
            offsets: vec![0.0; target_count],
            values: vec![0.0; target_count],
        }
    }

    pub fn target_indices(&self) -> &[usize] {
        &self.targets
    }

    pub fn last_values(&self) -> &[f64] {
        &self.last_values
    }

    /// Advance one frame and return each target's clamped value, in `target_indices` order.
    /// `sample` yields each source's normalized 0..1 value for this frame.
    pub fn step(&mut self, sample: impl Fn(Source) -> f32) -> &[f64] {
        self.offsets.fill(0.0);
        for (index, modulation) in self.modulations.iter().enumerate() {
            let mut x = sample(modulation.source);
            x = self.smoothers[index].process(x);
            if modulation.invert {
                x = 1.0 - x;
            }
            x = modulation.curve.apply(x);
            let value = if self.is_bool[index] {
                if x >= modulation.threshold { 1.0 } else { 0.0 }
            } else {
                lerp(modulation.range, x) as f64
            };
            let target = self.slots[index];
            self.offsets[target] += value - self.base_values[target];
        }
        for (target, &(min, max)) in self.bounds.iter().enumerate() {
            self.values[target] = (self.base_values[target] + self.offsets[target]).clamp(min, max);
        }
        &self.values
    }

    /// [`Self::step`], then `base` with every target applied. Records `last_values` as read back
    /// from the built effect.
    pub fn effect(
        &mut self,
        catalog: &ParamCatalog,
        base: &EffectSettings,
        sample: impl Fn(Source) -> f32,
    ) -> EffectSettings {
        self.step(sample);
        let mut effect = apply_targets(catalog, base, &self.targets, &self.values);
        for (target, &param_index) in self.targets.iter().enumerate() {
            self.last_values[target] = get_param(&effect, catalog.param(param_index));
        }
        skip_zero_noise(&mut effect);
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

/// Prune only stateless, exactly-zero noise on the already-owned frame settings.
/// ntsc-rs seeds each pass independently, so this does not advance another pass's RNG.
fn skip_zero_noise(effect: &mut EffectSettings) {
    for noise in [
        &mut effect.ntsc.composite_noise,
        &mut effect.ntsc.luma_noise,
        &mut effect.ntsc.chroma_noise,
    ] {
        if noise.settings.intensity == 0.0 {
            noise.enabled = false;
        }
    }
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
        let mut modulator = Modulator::new(catalog, mapping, fps);
        let mut values = Vec::with_capacity(features.frames * modulator.targets.len());
        for frame in 0..features.frames {
            values.extend_from_slice(modulator.step(|source| features.value(source, frame)));
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
        let mut effect = if self.frames == 0 {
            self.base.clone()
        } else {
            let count = self.targets.len();
            let start = frame.min(self.frames - 1) * count;
            apply_targets(
                catalog,
                &self.base,
                &self.targets,
                &self.values[start..start + count],
            )
        };
        skip_zero_noise(&mut effect);
        effect
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

    #[test]
    fn zero_noise_pruning_preserves_images_in_every_field_mode() {
        use crate::effect::FrameProcessor;

        let catalog = ParamCatalog::new();
        let (w, h) = (32, 18);
        let input: Vec<u8> = (0..w * h * 3).map(|i| (i * 37 % 256) as u8).collect();
        let timeline = features(vec![0.0; 3], vec![0.0; 3]);
        let empty = features(vec![], vec![]);
        for mode in 0..6 {
            // Every combination of zero/nonzero passes tests independent RNG streams.
            for mask in 0..8 {
                let mut map = mapping(vec![]);
                set_param(
                    &mut map.base,
                    catalog.get("use_field").unwrap(),
                    mode as f64,
                )
                .unwrap();
                map.base.ntsc.random_seed = 12345;
                for (index, noise) in [
                    &mut map.base.ntsc.composite_noise,
                    &mut map.base.ntsc.luma_noise,
                    &mut map.base.ntsc.chroma_noise,
                ]
                .into_iter()
                .enumerate()
                {
                    noise.enabled = true;
                    noise.settings.intensity = if mask & (1 << index) != 0 {
                        0.05
                    } else if index == 1 {
                        -0.0
                    } else {
                        0.0
                    };
                }
                let schedule = Schedule::build(&catalog, &map, &timeline, 30.0);
                let empty_schedule = Schedule::build(&catalog, &map, &empty, 30.0);
                let mut modulator = Modulator::new(&catalog, &map, 30.0);
                let mut reference = FrameProcessor::new(w, h, map.chain);
                let mut optimized = FrameProcessor::new(w, h, map.chain);
                let mut expected = vec![0; input.len()];
                let mut actual = vec![0; input.len()];
                for frame in [0, 1, 17] {
                    reference.process(&map.base, frame, &input, &mut expected);
                    for effect in [
                        modulator.effect(&catalog, &map.base, |_| 0.0),
                        schedule.effect(&catalog, frame),
                        empty_schedule.effect(&catalog, frame),
                    ] {
                        for (index, noise) in [
                            &effect.ntsc.composite_noise,
                            &effect.ntsc.luma_noise,
                            &effect.ntsc.chroma_noise,
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            assert_eq!(noise.enabled, mask & (1 << index) != 0);
                        }
                        optimized.process(&effect, frame, &input, &mut actual);
                        assert_eq!(actual, expected, "mode {mode}, mask {mask}, frame {frame}");
                    }
                }
            }
        }
    }

    #[test]
    fn noise_modulation_can_resume_after_zero_without_changing_reported_targets() {
        let catalog = ParamCatalog::new();
        let map = mapping(vec![
            m(&catalog, "luma_noise_intensity", Source::Rms, (0.0, 0.05)),
            m(&catalog, "luma_noise", Source::Onset, (0.0, 1.0)),
        ]);
        let mut modulator = Modulator::new(&catalog, &map, 30.0);
        for sample in [0.0, 1.0, 0.0, 1.0] {
            let effect = modulator.effect(&catalog, &map.base, |source| match source {
                Source::Rms => sample,
                _ => 1.0,
            });
            assert_eq!(effect.ntsc.luma_noise.enabled, sample != 0.0);
            assert_eq!(modulator.last_values()[1], 1.0);
            assert!(map.base.ntsc.luma_noise.enabled);
        }
        let mut disabled_map = mapping(vec![]);
        disabled_map.base.ntsc.luma_noise.enabled = false;
        let mut modulator = Modulator::new(&catalog, &disabled_map, 30.0);
        let effect = modulator.effect(&catalog, &disabled_map.base, |_| 1.0);
        assert!(!effect.ntsc.luma_noise.enabled);
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
        let mut mo = Modulator::new(&c, &map, 30.0);
        let p = c.get("tracking_noise_wave_intensity").unwrap();
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 0)), p),
            2.0
        );
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 1)), p),
            16.0
        );
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 2)), p),
            30.0
        );
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
        let mut mo = Modulator::new(&c, &map, 30.0);
        let effect = mo.effect(&c, &map.base, |s| f.value(s, 0));
        assert_eq!(effect.bender.sync.settings.v_roll, 0.0);
        let effect = mo.effect(&c, &map.base, |s| f.value(s, 1));
        assert_eq!(effect.bender.sync.settings.v_roll, 0.5);
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
        let mut mo = Modulator::new(&c, &map, 30.0);
        let value = get_param(&mo.effect(&c, &map.base, |s| f.value(s, 0)), p);
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
        let mut mo = Modulator::new(&c, &map, 30.0);
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 0)), p),
            100.0
        );
        let v = get_param(&mo.effect(&c, &map.base, |s| f.value(s, 1)), p);
        assert!((v - 40.0).abs() < 0.01, "got {v}");
    }

    #[test]
    fn bool_threshold_and_onset_release() {
        let c = ParamCatalog::new();
        let mut hs = m(&c, "head_switching", Source::Onset, (0.0, 1.0));
        hs.release_ms = 200.0;
        let map = mapping(vec![hs]);
        let f = features(vec![0.0; 6], vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        let mut mo = Modulator::new(&c, &map, 30.0);
        let p = c.get("head_switching").unwrap();
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 0)), p),
            0.0
        );
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 1)), p),
            1.0
        );
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 2)), p),
            1.0,
            "still above 0.5 one frame later at 200ms release"
        );
        for fr in 3..6 {
            mo.effect(&c, &map.base, |s| f.value(s, fr));
        }
        let f2 = features(vec![0.0; 40], vec![0.0; 40]);
        let mut mo2 = Modulator::new(&c, &map, 30.0);
        assert_eq!(
            get_param(&mo2.effect(&c, &map.base, |s| f2.value(s, 0)), p),
            0.0
        );
    }

    #[test]
    fn invert_and_enum_rounding() {
        let c = ParamCatalog::new();
        let mut e = m(&c, "vhs_tape_speed", Source::Band(0), (0.0, 3.0));
        e.invert = true;
        let map = mapping(vec![e]);
        let f = features(vec![0.0, 0.4], vec![0.0; 2]);
        let mut mo = Modulator::new(&c, &map, 30.0);
        let p = c.get("vhs_tape_speed").unwrap();
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 0)), p),
            3.0
        );
        assert_eq!(
            get_param(&mo.effect(&c, &map.base, |s| f.value(s, 1)), p),
            2.0
        );
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
        let mut sequential = Modulator::new(&c, &map, 30.0);
        let rendered: Vec<_> = (0..f.frames)
            .map(|frame| sequential.effect(&c, &map.base, |s| f.value(s, frame)))
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
