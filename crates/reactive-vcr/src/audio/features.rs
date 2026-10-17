#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{Normalize, Source};

    fn params(frames: usize) -> FeatureParams {
        FeatureParams {
            fps: 30.0,
            frames,
            start_s: 0.0,
            bands: vec![[20.0, 200.0], [3000.0, 12000.0]],
            normalize: Normalize::Peak,
            onset_sensitivity: 1.5,
        }
    }

    fn sine(freq: f32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| {
                amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    #[test]
    fn bass_sine_lights_bass_band_only() {
        let s = sine(100.0, 1.0, 0.5);
        let t = extract(&s, SAMPLE_RATE, &params(30));
        assert_eq!(t.frames, 30);
        let mid = 15;
        assert!(t.bands[0][mid] > 0.9, "bass {}", t.bands[0][mid]);
        assert!(t.bands[1][mid] < 0.2, "high {}", t.bands[1][mid]);
        assert!(t.rms[mid] > 0.9, "rms {}", t.rms[mid]);
        assert!(t.value(Source::Band(0), 999) > 0.9, "clamps frame index");
    }

    #[test]
    fn silence_is_zero_everywhere() {
        let s = vec![0.0f32; SAMPLE_RATE as usize];
        let t = extract(&s, SAMPLE_RATE, &params(30));
        assert!(t.rms.iter().all(|v| *v == 0.0));
        assert!(t.onset.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn click_train_yields_onsets_at_click_frames() {
        let mut s = vec![0.0f32; 2 * SAMPLE_RATE as usize];
        let mut seed = 12345u32;
        for t in [0.5f32, 1.2] {
            let start = (t * SAMPLE_RATE as f32) as usize;
            for i in 0..480 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                s[start + i] = (seed >> 8) as f32 / (1u32 << 24) as f32 * 1.8 - 0.9;
            }
        }
        let t = extract(&s, SAMPLE_RATE, &params(60));
        let hits: Vec<usize> = t
            .onset
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0.0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(hits.len(), 2, "hits {hits:?}");
        assert!(
            (hits[0] as i64 - 15).abs() <= 1,
            "first hit at frame {}",
            hits[0]
        );
        assert!(
            (hits[1] as i64 - 36).abs() <= 1,
            "second hit at frame {}",
            hits[1]
        );
    }

    #[test]
    fn normalize_maps_60db_window() {
        let db = vec![-100.0f32, -60.0, -30.0, 0.0];
        let n = normalize_db(&db, Normalize::Peak);
        assert_eq!(n[0], 0.0);
        assert_eq!(n[1], 0.0);
        assert!((n[2] - 0.5).abs() < 1e-6);
        assert_eq!(n[3], 1.0);
        let n = normalize_db(&db, Normalize::Percentile(50.0));
        assert_eq!(n[3], 1.0, "values above the reference clamp to 1");
        assert!(n[2] > 0.5);
        assert_eq!(reference_db(&db, Normalize::Peak), 0.0);
        assert_eq!(normalize_db_with(&[-90.0, -70.0], 0.0), vec![0.0, 0.0]);
        assert!(
            normalize_db_with(&[-120.0; 3], -120.0)
                .iter()
                .all(|v| *v == 0.0),
            "silence stays zero"
        );
    }

    #[test]
    fn percentile_floor_falls_back_to_peak_for_sparse_audio() {
        let db = [-120.0, -120.0, -120.0, -20.0];
        assert_eq!(
            normalize_db(&db, Normalize::Percentile(50.0)),
            vec![0.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn silence_stays_zero_alongside_quiet_audio() {
        assert_eq!(normalize_db_with(&[-120.0, -100.0], -100.0), vec![0.0, 1.0]);
    }

    #[test]
    fn quiet_signal_above_digital_floor_normalizes_to_peak() {
        let samples = vec![1.06e-6; SAMPLE_RATE as usize];
        let timeline = extract(&samples, SAMPLE_RATE, &params(30));
        assert_eq!(timeline.rms[15], 1.0);
        assert_eq!(normalize_db(&[-119.5; 3], Normalize::Peak), vec![1.0; 3]);
    }

    #[test]
    fn percentile_above_digital_floor_keeps_its_reference() {
        let db = [-119.8, -119.5, -119.5, -20.0];
        assert_eq!(reference_db(&db, Normalize::Percentile(50.0)), -119.5);
        let normalized = normalize_db(&db, Normalize::Percentile(50.0));
        assert!((normalized[0] - 0.995).abs() < 1e-6);
        assert_eq!(&normalized[1..], &[1.0, 1.0, 1.0]);
    }

    #[test]
    fn frames_for_counts_frames_in_range() {
        assert_eq!(
            frames_for(SAMPLE_RATE as usize * 2, SAMPLE_RATE, 30.0, 0.0, None),
            60
        );
        assert_eq!(
            frames_for(SAMPLE_RATE as usize * 2, SAMPLE_RATE, 30.0, 1.0, None),
            30
        );
        assert_eq!(
            frames_for(SAMPLE_RATE as usize * 2, SAMPLE_RATE, 30.0, 0.5, Some(0.5)),
            15
        );
    }
}
use crate::mapping::{Normalize, Source};
use realfft::RealFftPlanner;

/// Mono analysis sample rate; decoders resample to it.
pub const SAMPLE_RATE: u32 = 48_000;
pub const FFT_SIZE: usize = 2048;
const DB_WINDOW: f32 = 60.0;
const DB_FLOOR: f32 = -120.0;
const ONSET_MIN_GAP: usize = 3;
const ONSET_CONTEXT: usize = 10;

#[derive(Debug, Clone)]
pub struct FeatureParams {
    pub fps: f64,
    pub frames: usize,
    pub start_s: f64,
    pub bands: Vec<[f32; 2]>,
    pub normalize: Normalize,
    pub onset_sensitivity: f32,
}

#[derive(Debug, Clone)]
pub struct FeatureTimeline {
    pub frames: usize,
    pub rms: Vec<f32>,
    pub bands: Vec<Vec<f32>>,
    /// 1.0 on onset frames, else 0.0.
    pub onset: Vec<f32>,
}

impl FeatureTimeline {
    pub fn value(&self, source: Source, frame: usize) -> f32 {
        if self.frames == 0 {
            return 0.0;
        }
        let frame = frame.min(self.frames - 1);
        match source {
            Source::Rms => self.rms[frame],
            Source::Band(index) => self.bands[index][frame],
            Source::Onset => self.onset[frame],
        }
    }
}

/// Number of video frames covered by the audio from `start_s`, optionally capped by `duration_s`.
pub fn frames_for(
    samples: usize,
    sample_rate: u32,
    fps: f64,
    start_s: f64,
    duration_s: Option<f64>,
) -> usize {
    let total_s = samples as f64 / sample_rate as f64;
    let mut span = (total_s - start_s).max(0.0);
    if let Some(duration_s) = duration_s {
        span = span.min(duration_s);
    }
    (span * fps).round() as usize
}

fn db_power(power: f32) -> f32 {
    10.0 * power.max(1e-12).log10()
}

/// Reference level in dB for a feature: its peak or the given percentile.
pub fn reference_db(db: &[f32], mode: Normalize) -> f32 {
    if db.is_empty() {
        return DB_FLOOR;
    }
    let peak = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    match mode {
        Normalize::Peak => peak,
        Normalize::Percentile(percentile) => {
            let mut sorted = db.to_vec();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let index = ((percentile / 100.0) * (sorted.len() - 1) as f32).round() as usize;
            let reference = sorted[index.min(sorted.len() - 1)];
            // A mostly silent stream can put the percentile at the digital floor.
            // Preserve its actual bursts while keeping genuinely silent streams at zero.
            if reference <= DB_FLOOR && peak > reference {
                peak
            } else {
                reference
            }
        }
    }
}

/// Map dB values to 0..1 over a 60 dB window below `reference`.
pub fn normalize_db_with(db: &[f32], reference: f32) -> Vec<f32> {
    if reference <= DB_FLOOR {
        return vec![0.0; db.len()];
    }
    let floor = reference - DB_WINDOW;
    db.iter()
        .map(|value| {
            if *value <= DB_FLOOR {
                0.0
            } else {
                ((value - floor) / DB_WINDOW).clamp(0.0, 1.0)
            }
        })
        .collect()
}

pub fn normalize_db(db: &[f32], mode: Normalize) -> Vec<f32> {
    normalize_db_with(db, reference_db(db, mode))
}

fn pick_onsets(flux: &[f32], sensitivity: f32) -> Vec<f32> {
    let mut onsets = vec![0.0; flux.len()];
    let mut last_onset = None;
    for index in 0..flux.len() {
        let previous = if index == 0 { 0.0 } else { flux[index - 1] };
        let next = flux.get(index + 1).copied().unwrap_or(0.0);
        let start = index.saturating_sub(ONSET_CONTEXT);
        let end = (index + ONSET_CONTEXT + 1).min(flux.len());
        let mean = flux[start..end].iter().sum::<f32>() / (end - start) as f32;
        let is_peak = flux[index] > previous
            && flux[index] >= next
            && flux[index] > sensitivity * mean
            && flux[index] > 1e-6;
        let has_gap = last_onset.is_none_or(|last| index - last >= ONSET_MIN_GAP);
        if is_peak && has_gap {
            onsets[index] = 1.0;
            last_onset = Some(index);
        }
    }
    onsets
}

pub fn extract(samples: &[f32], sample_rate: u32, p: &FeatureParams) -> FeatureTimeline {
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|index| {
            0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / FFT_SIZE as f32).cos()
        })
        .collect();
    let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
    let mut rms_db = vec![DB_FLOOR; p.frames];
    let mut band_db = vec![vec![DB_FLOOR; p.frames]; p.bands.len()];
    let mut flux = vec![0.0; p.frames];
    let mut previous_magnitudes = vec![0.0; spectrum.len()];
    let mut magnitudes = vec![0.0; spectrum.len()];

    for frame in 0..p.frames {
        let center =
            (((frame as f64 + 0.5) / p.fps + p.start_s) * sample_rate as f64).round() as i64;
        let start = center - (FFT_SIZE / 2) as i64;
        let mut sum_of_squares = 0.0;
        for index in 0..FFT_SIZE {
            let sample_index = start + index as i64;
            let sample = if sample_index >= 0 && (sample_index as usize) < samples.len() {
                samples[sample_index as usize]
            } else {
                0.0
            };
            sum_of_squares += sample * sample;
            input[index] = sample * window[index];
        }
        rms_db[frame] = 20.0 * (sum_of_squares / FFT_SIZE as f32).sqrt().max(1e-6).log10();

        fft.process(&mut input, &mut spectrum)
            .expect("FFT input and output sizes match");
        for (index, value) in spectrum.iter().enumerate() {
            magnitudes[index] = value.norm();
        }
        for (band_index, [low_hz, high_hz]) in p.bands.iter().enumerate() {
            let first_bin = (low_hz / bin_hz).ceil() as usize;
            let end_bin = ((high_hz / bin_hz).ceil() as usize).min(magnitudes.len());
            let start_bin = first_bin.min(end_bin);
            let power = magnitudes[start_bin..end_bin]
                .iter()
                .map(|magnitude| magnitude * magnitude)
                .sum();
            band_db[band_index][frame] = db_power(power);
        }
        flux[frame] = magnitudes
            .iter()
            .zip(&previous_magnitudes)
            .map(|(current, previous)| (current - previous).max(0.0))
            .sum();
        std::mem::swap(&mut magnitudes, &mut previous_magnitudes);
    }

    let band_reference = reference_db(&band_db.concat(), p.normalize);
    FeatureTimeline {
        frames: p.frames,
        rms: normalize_db(&rms_db, p.normalize),
        bands: band_db
            .iter()
            .map(|band| normalize_db_with(band, band_reference))
            .collect(),
        onset: pick_onsets(&flux, p.onset_sensitivity),
    }
}
