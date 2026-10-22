//! Causal analysis for live input: each frame sees only audio that has already arrived.
//!
//! Results intentionally differ from offline [`crate::audio::features::extract`], which centers
//! each window on its frame, normalizes against the whole file, and picks onsets with lookahead.
//! Here the window trails the newest sample, levels normalize against a trailing
//! [`LIVE_REFERENCE_WINDOW_S`] window, and onsets fire one frame after their flux peak.
use crate::audio::features::{
    DB_FLOOR, FFT_SIZE, ONSET_CONTEXT, ONSET_MIN_GAP, SpectralFrame, normalize_db_value,
    reference_db_with,
};
use crate::effect::{ChainLayout, EffectSettings};
use crate::mapping::{CompiledMapping, Normalize, Source};
use crate::modulate::Modulator;
use crate::params::ParamCatalog;
use std::collections::VecDeque;
use std::sync::Arc;

/// Seconds of past levels that set the live normalization reference.
pub const LIVE_REFERENCE_WINDOW_S: f64 = 10.0;

/// Normalized 0..1 features for one live frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameFeatures {
    pub rms: f32,
    pub bands: Vec<f32>,
    /// 1.0 on onset frames, else 0.0.
    pub onset: f32,
}

impl FrameFeatures {
    pub fn value(&self, source: Source) -> f32 {
        match source {
            Source::Rms => self.rms,
            Source::Band(index) => self.bands[index],
            Source::Onset => self.onset,
        }
    }
}

/// Per-frame features from a trailing window of live mono audio.
pub struct LiveAnalyzer {
    spectral: SpectralFrame,
    bands: Vec<[f32; 2]>,
    normalize: Normalize,
    onset_sensitivity: f32,
    /// The newest `FFT_SIZE` samples; `head` indexes the oldest.
    ring: Vec<f32>,
    head: usize,
    window_frames: usize,
    band_db: Vec<f32>,
    rms_history: VecDeque<f32>,
    /// Frame-major band dB values for the last `window_frames` frames.
    band_history: VecDeque<f32>,
    flux_history: VecDeque<f32>,
    frame: usize,
    last_onset: Option<usize>,
    sort_scratch: Vec<f32>,
    features: FrameFeatures,
}

impl LiveAnalyzer {
    pub fn new(mapping: &CompiledMapping, sample_rate: u32, fps: f64) -> Self {
        let bands: Vec<[f32; 2]> = mapping.bands.iter().map(|(_, range)| *range).collect();
        let window_frames = ((LIVE_REFERENCE_WINDOW_S * fps).round() as usize).max(1);
        let band_count = bands.len();
        Self {
            spectral: SpectralFrame::new(sample_rate),
            normalize: mapping.normalize,
            onset_sensitivity: mapping.onset_sensitivity,
            ring: vec![0.0; FFT_SIZE],
            head: 0,
            window_frames,
            band_db: vec![DB_FLOOR; band_count],
            rms_history: VecDeque::with_capacity(window_frames + 1),
            band_history: VecDeque::with_capacity((window_frames + 1) * band_count),
            flux_history: VecDeque::with_capacity(ONSET_CONTEXT + 3),
            frame: 0,
            last_onset: None,
            sort_scratch: Vec::new(),
            features: FrameFeatures {
                rms: 0.0,
                bands: vec![0.0; band_count],
                onset: 0.0,
            },
            bands,
        }
    }

    /// Append mono samples at the analyzer's sample rate; only the newest `FFT_SIZE` are kept.
    pub fn push(&mut self, mono: &[f32]) {
        let mono = &mono[mono.len().saturating_sub(FFT_SIZE)..];
        for &sample in mono {
            self.ring[self.head] = sample;
            self.head = (self.head + 1) % FFT_SIZE;
        }
    }

    /// Features for one video frame from the newest `FFT_SIZE` samples. Call exactly once per
    /// frame: normalization and onset detection advance with every call.
    pub fn analyze(&mut self) -> &FrameFeatures {
        let (ring, head) = (&self.ring, self.head);
        let (rms_db, flux) = self.spectral.analyze(
            |index| ring[(head + index) % FFT_SIZE],
            &self.bands,
            &mut self.band_db,
        );

        self.rms_history.push_back(rms_db);
        if self.rms_history.len() > self.window_frames {
            self.rms_history.pop_front();
        }
        self.band_history.extend(self.band_db.iter().copied());
        while self.band_history.len() > self.window_frames * self.bands.len() {
            self.band_history.pop_front();
        }
        let rms_reference = reference_db_with(
            self.rms_history.make_contiguous(),
            self.normalize,
            &mut self.sort_scratch,
        );
        self.features.rms = normalize_db_value(rms_db, rms_reference);
        let band_reference = reference_db_with(
            self.band_history.make_contiguous(),
            self.normalize,
            &mut self.sort_scratch,
        );
        for (normalized, &db) in self.features.bands.iter_mut().zip(&self.band_db) {
            *normalized = normalize_db_value(db, band_reference);
        }

        self.features.onset = if self.onset_after(flux) { 1.0 } else { 0.0 };
        self.frame += 1;
        &self.features
    }

    /// Record this frame's flux and judge whether the previous frame was an onset peak.
    fn onset_after(&mut self, flux: f32) -> bool {
        self.flux_history.push_back(flux);
        if self.flux_history.len() > ONSET_CONTEXT + 2 {
            self.flux_history.pop_front();
        }
        let len = self.flux_history.len();
        if len < 2 {
            return false;
        }
        let next = self.flux_history[len - 1];
        let current = self.flux_history[len - 2];
        let previous = if len >= 3 {
            self.flux_history[len - 3]
        } else {
            0.0
        };
        let mean = self.flux_history.iter().sum::<f32>() / len as f32;
        let is_peak = current > previous
            && current >= next
            && current > self.onset_sensitivity * mean
            && current > 1e-6;
        let has_gap = self
            .last_onset
            .is_none_or(|last| self.frame - last >= ONSET_MIN_GAP);
        let fired = is_peak && has_gap;
        if fired {
            self.last_onset = Some(self.frame);
        }
        fired
    }

    /// Carry buffered audio over from `previous`; level and onset histories restart.
    fn keep_audio_from(&mut self, previous: &LiveAnalyzer) {
        self.ring.copy_from_slice(&previous.ring);
        self.head = previous.head;
    }
}

/// Live analysis plus modulation: turns pushed audio into one [`EffectSettings`] per frame.
pub struct LiveProgram {
    catalog: Arc<ParamCatalog>,
    mapping: Arc<CompiledMapping>,
    sample_rate: u32,
    fps: f64,
    analyzer: LiveAnalyzer,
    modulator: Modulator,
    frame: usize,
}

impl LiveProgram {
    pub fn new(
        catalog: Arc<ParamCatalog>,
        mapping: Arc<CompiledMapping>,
        sample_rate: u32,
        fps: f64,
    ) -> Self {
        let analyzer = LiveAnalyzer::new(&mapping, sample_rate, fps);
        let modulator = Modulator::new(&catalog, &mapping, fps);
        Self {
            catalog,
            mapping,
            sample_rate,
            fps,
            analyzer,
            modulator,
            frame: 0,
        }
    }

    /// Append mono samples at the program's sample rate.
    pub fn push_audio(&mut self, mono: &[f32]) {
        self.analyzer.push(mono);
    }

    /// Analyze the newest audio, advance modulation one frame, and return the frame number and
    /// its settings.
    pub fn next_frame(&mut self) -> (usize, EffectSettings) {
        let features = self.analyzer.analyze();
        let effect = self
            .modulator
            .effect(&self.catalog, &self.mapping.base, |source| {
                features.value(source)
            });
        let frame = self.frame;
        self.frame += 1;
        (frame, effect)
    }

    pub fn chain(&self) -> ChainLayout {
        self.mapping.chain
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// The normalized features the latest frame's modulations read; zeros before the first
    /// frame and after a rebuild.
    pub fn features(&self) -> &FrameFeatures {
        &self.analyzer.features
    }

    /// The mapping's band names, in the order of [`FrameFeatures::bands`].
    pub fn band_names(&self) -> impl Iterator<Item = &str> {
        self.mapping.bands.iter().map(|(name, _)| name.as_str())
    }

    /// Each modulated target's name, as a mapping writes it, and its value as applied to the
    /// latest frame.
    pub fn targets(&self) -> impl Iterator<Item = (&'static str, f64)> + '_ {
        self.modulator
            .target_indices()
            .iter()
            .zip(self.modulator.last_values())
            .map(|(&index, &value)| (self.catalog.param(index).name, value))
    }

    /// Rebuild analysis and modulation for a new frame rate; a no-op if unchanged.
    pub fn set_fps(&mut self, fps: f64) {
        if fps != self.fps {
            self.fps = fps;
            self.rebuild();
        }
    }

    /// Rebuild analysis and modulation for a reloaded mapping, keeping buffered audio and the
    /// frame counter.
    pub fn set_mapping(&mut self, mapping: Arc<CompiledMapping>) {
        self.mapping = mapping;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let mut analyzer = LiveAnalyzer::new(&self.mapping, self.sample_rate, self.fps);
        analyzer.keep_audio_from(&self.analyzer);
        self.analyzer = analyzer;
        self.modulator = Modulator::new(&self.catalog, &self.mapping, self.fps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{MappingFile, compile};
    use crate::params::get_param;
    use std::path::Path;

    const RATE: u32 = 48_000;
    const PER_FRAME: usize = 1600;

    fn mapping(audio: &str, modulations: &str) -> CompiledMapping {
        let text = format!("[audio]\n{audio}\n{modulations}");
        let file = MappingFile::parse(&text).unwrap();
        compile(file, Path::new("."), &ParamCatalog::new()).unwrap()
    }

    fn bands_mapping(normalize: &str) -> CompiledMapping {
        mapping(
            &format!(
                "bands.bass = [20, 200]\nbands.high = [3000, 12000]\nnormalize = \"{normalize}\"\nonset_sensitivity = 1.5"
            ),
            "",
        )
    }

    /// A continuous sine; `start` is the absolute index of the first sample.
    fn sine(start: usize, len: usize, freq: f32, amplitude: f32) -> Vec<f32> {
        (start..start + len)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn trailing_window_sees_only_pushed_audio() {
        let map = bands_mapping("peak");
        let mut silent = LiveAnalyzer::new(&map, RATE, 30.0);
        let f = silent.analyze();
        assert_eq!(f.rms, 0.0);
        assert!(f.bands.iter().all(|&b| b == 0.0), "{:?}", f.bands);
        assert_eq!(f.onset, 0.0);

        let mut loud = LiveAnalyzer::new(&map, RATE, 30.0);
        loud.push(&sine(0, FFT_SIZE, 110.0, 0.8));
        let f = loud.analyze();
        assert!(f.rms > 0.9, "rms {}", f.rms);
        assert!(f.bands[0] > f.bands[1], "bands {:?}", f.bands);
    }

    #[test]
    fn adaptive_reference_tracks_level_changes() {
        let map = bands_mapping("peak");
        let mut analyzer = LiveAnalyzer::new(&map, RATE, 30.0);
        let mut at = 0;
        for _ in 0..60 {
            analyzer.push(&sine(at, PER_FRAME, 110.0, 0.8));
            at += PER_FRAME;
            analyzer.analyze();
        }
        for quiet in 1..=10 {
            analyzer.push(&sine(at, PER_FRAME, 110.0, 0.008));
            at += PER_FRAME;
            let rms = analyzer.analyze().rms;
            if quiet >= 3 {
                assert!((rms - 1.0 / 3.0).abs() < 0.05, "quiet frame {quiet}: {rms}");
            }
        }
        let settle = (LIVE_REFERENCE_WINDOW_S * 30.0) as usize + 30;
        let mut rms = 0.0;
        for _ in 0..settle {
            analyzer.push(&sine(at, PER_FRAME, 110.0, 0.008));
            at += PER_FRAME;
            rms = analyzer.analyze().rms;
        }
        assert!(rms > 0.9, "adapted rms {rms}");
    }

    #[test]
    fn onset_fires_one_frame_after_the_peak() {
        let map = bands_mapping("peak");
        let mut analyzer = LiveAnalyzer::new(&map, RATE, 30.0);
        let click = |chunk: &mut [f32]| {
            for (i, sample) in chunk[..64].iter_mut().enumerate() {
                *sample = if i % 2 == 0 { 0.9 } else { -0.9 };
            }
        };
        let (first, second) = (20, 22);
        let mut onsets = Vec::new();
        for frame in 0..40 {
            let mut chunk = vec![0.0; PER_FRAME];
            if frame == first || frame == second {
                click(&mut chunk);
            }
            analyzer.push(&chunk);
            onsets.push(analyzer.analyze().onset);
        }
        for (frame, &onset) in onsets.iter().enumerate() {
            let expected = if frame == first + 1 { 1.0 } else { 0.0 };
            assert_eq!(onset, expected, "frame {frame}");
        }
    }

    #[test]
    fn set_mapping_keeps_buffered_audio() {
        let catalog = Arc::new(ParamCatalog::new());
        let map = Arc::new(mapping(
            "normalize = \"peak\"",
            "[[modulate]]\ntarget = \"snow_intensity\"\nsource = \"rms\"\nrange = [0.0, 2.0]",
        ));
        let mut program = LiveProgram::new(catalog.clone(), map.clone(), RATE, 30.0);
        program.push_audio(&sine(0, FFT_SIZE, 110.0, 0.8));
        program.set_mapping(Arc::new((*map).clone()));
        let (frame, effect) = program.next_frame();
        assert_eq!(frame, 0);
        let snow = get_param(&effect, catalog.get("snow_intensity").unwrap());
        assert!(snow > 1.5, "snow {snow}");
    }
}
