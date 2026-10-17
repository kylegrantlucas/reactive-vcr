mod common;

use common::{clip, song};
use reactive_vcr_gst::decode::decode_mono_f32;
use reactive_vcr_gst::probe::probe;

#[test]
fn probe_reports_size_rate_duration_and_audio() {
    let dir = tempfile::tempdir().unwrap();
    let info = probe(&clip(dir.path(), 2, true)).unwrap();
    assert_eq!((info.width, info.height), (160, 120));
    assert_eq!((info.fps_n, info.fps_d), (30, 1));
    assert!((info.duration_s - 2.0).abs() < 0.05, "{}", info.duration_s);
    assert!(info.has_audio);

    let silent = tempfile::tempdir().unwrap();
    assert!(!probe(&clip(silent.path(), 1, false)).unwrap().has_audio);
}

#[test]
fn probe_rejects_audio_only_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let error = probe(&song(dir.path(), 1)).unwrap_err();
    assert!(
        format!("{error:#}").contains("no video stream"),
        "{error:#}"
    );
    let error = probe(&dir.path().join("missing.mp4")).unwrap_err();
    assert!(format!("{error:#}").contains("not found"), "{error:#}");
}

#[test]
fn decodes_stereo_44k_to_48k_mono() {
    let dir = tempfile::tempdir().unwrap();
    let samples = decode_mono_f32(&song(dir.path(), 1)).unwrap();
    assert!(
        (samples.len() as i64 - 48_000).abs() < 100,
        "{} samples",
        samples.len()
    );
    let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!(peak > 0.3 && peak <= 1.0, "peak {peak}");
}

#[test]
fn decodes_the_audio_of_a_video_file() {
    let dir = tempfile::tempdir().unwrap();
    let samples = decode_mono_f32(&clip(dir.path(), 1, true)).unwrap();
    assert!(
        (samples.len() as i64 - 48_000).abs() < 2_000,
        "{}",
        samples.len()
    );
}

#[test]
fn decoding_fails_without_an_audio_stream() {
    let dir = tempfile::tempdir().unwrap();
    let error = decode_mono_f32(&clip(dir.path(), 1, false)).unwrap_err();
    assert!(
        format!("{error:#}").contains("no audio stream"),
        "{error:#}"
    );
    let error = decode_mono_f32(&dir.path().join("missing.wav")).unwrap_err();
    assert!(format!("{error:#}").contains("not found"), "{error:#}");
}
