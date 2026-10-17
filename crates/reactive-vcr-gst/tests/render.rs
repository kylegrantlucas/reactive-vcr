mod common;

use common::{Render, audio_seconds, clip, example, render, rgb_frames, song};
use reactive_vcr_gst::AudioMode;
use reactive_vcr_gst::render::{OutputCodec, RenderJob, RenderStatus};

fn job(dir: &std::path::Path, map: std::path::PathBuf, output: &str) -> Render {
    Render {
        video: clip(dir, 3, true),
        audio: song(dir, 3),
        map,
        output: dir.join(output),
        codec: OutputCodec::Ffv1,
        audio_mode: AudioMode::Mix,
        start: None,
        duration: None,
    }
}

#[test]
fn renders_every_frame_with_mixed_audio() {
    let dir = tempfile::tempdir().unwrap();
    let r = job(dir.path(), example("basic.toml"), "out.mkv");
    assert_eq!(render(&r).unwrap(), 90);
    let frames = rgb_frames(&r.output);
    assert_eq!(frames.len(), 90);
    let source = rgb_frames(&r.video);
    assert_ne!(frames[45], source[45], "the effect should alter frames");
    let audio = audio_seconds(&r.output).expect("mixed audio track");
    assert!((audio - 3.0).abs() < 0.05, "audio lasts {audio}s");
}

#[test]
fn selected_range_starts_at_the_requested_source_frame() {
    let dir = tempfile::tempdir().unwrap();
    let passthrough = dir.path().join("pass.toml");
    std::fs::write(&passthrough, "[base]\nntsc = false\n").unwrap();
    let mut r = job(dir.path(), passthrough, "out.mkv");
    r.start = Some(1.0);
    r.duration = Some(1.5);
    r.audio_mode = AudioMode::Driving;
    assert_eq!(render(&r).unwrap(), 45);
    let frames = rgb_frames(&r.output);
    let source = rgb_frames(&r.video);
    assert_eq!(frames.len(), 45);
    assert!(
        frames[0] == source[30],
        "first frame is the source frame at 1.0s"
    );
    assert!(
        frames[44] == source[74],
        "last frame is the source frame at 2.467s"
    );
    assert!(audio_seconds(&r.output).is_some());
}

#[test]
fn audio_mode_none_writes_no_audio_track() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = job(dir.path(), example("basic.toml"), "out.mkv");
    r.duration = Some(1.0);
    r.audio_mode = AudioMode::None;
    assert_eq!(render(&r).unwrap(), 30);
    assert_eq!(audio_seconds(&r.output), None);
}

#[test]
fn driving_audio_is_padded_to_the_video_length() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = job(dir.path(), example("basic.toml"), "out.mkv");
    r.audio = song(&subdir(dir.path(), "short"), 2);
    r.audio_mode = AudioMode::Driving;
    render(&r).unwrap();
    let audio = audio_seconds(&r.output).expect("driving audio track");
    assert!((audio - 3.0).abs() < 0.05, "audio lasts {audio}s");
}

#[test]
fn renders_h264_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = job(dir.path(), example("bender.toml"), "out.mp4");
    r.codec = OutputCodec::H264 { quantizer: 18 };
    assert_eq!(render(&r).unwrap(), 90);
    assert_eq!(rgb_frames(&r.output).len(), 90);
    assert!(audio_seconds(&r.output).is_some());
}

#[test]
fn output_may_not_alias_an_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = job(dir.path(), example("basic.toml"), "unused.mkv");
    r.output = r.video.clone();
    let before = std::fs::read(&r.video).unwrap();
    let error = render(&r).unwrap_err();
    assert!(
        format!("{error:#}").contains("aliases video input"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&r.video).unwrap(), before);
}

#[test]
fn cancel_stops_and_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = job(dir.path(), example("basic.toml"), "out.mkv");
    r.video = clip(&subdir(dir.path(), "long"), 20, true);
    let job = RenderJob::start(common::settings(&r).unwrap()).unwrap();
    job.cancel();
    assert_eq!(job.status(), RenderStatus::Cancelled);
    assert!(!r.output.exists());
}

fn subdir(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}
