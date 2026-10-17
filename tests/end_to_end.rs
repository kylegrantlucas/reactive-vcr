use reactive_vcr::audio::decode::tool_available;
use reactive_vcr::render::{RenderOptions, render};
use reactive_vcr::video::encoder::AudioMode;
use std::process::Command;
use std::time::{Duration, Instant};

fn synth(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let clip = dir.join("in.mp4");
    let song = dir.join("song.wav");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x120:rate=30:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&clip)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=110:duration=2,tremolo=f=4:d=0.9",
            ])
            .arg(&song)
            .status()
            .unwrap()
            .success()
    );
    (clip, song)
}

fn probe_streams(path: &std::path::Path) -> (u64, usize) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-count_frames",
        ])
        .arg(path)
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let streams = value["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    let frames: u64 = video["nb_read_frames"].as_str().unwrap().parse().unwrap();
    let audio = streams
        .iter()
        .filter(|stream| stream["codec_type"] == "audio")
        .count();
    (frames, audio)
}

fn map() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/basic.toml")
}

#[test]
fn renders_modulated_clip_with_mixed_audio() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (clip, song) = synth(dir.path());
    let out = dir.path().join("out.mp4");
    let summary = render(&RenderOptions {
        video: clip,
        audio: song,
        map: map(),
        output: out.clone(),
        preview: Some(0.5),
        start_s: None,
        duration_s: None,
        audio_mode: AudioMode::Mix,
        extra_out_args: vec![],
        quiet: true,
    })
    .unwrap();
    assert_eq!((summary.width, summary.height), (80, 60));
    assert_eq!(summary.frames, 60);
    let (frames, audio_streams) = probe_streams(&out);
    assert_eq!(frames, 60);
    assert_eq!(audio_streams, 1);
}

#[test]
fn renders_bender_example_mapping() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (clip, song) = synth(dir.path());
    let out = dir.path().join("out.mp4");
    let summary = render(&RenderOptions {
        video: clip,
        audio: song,
        map: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/bender.toml"),
        output: out.clone(),
        preview: Some(0.5),
        start_s: None,
        duration_s: None,
        audio_mode: AudioMode::Mix,
        extra_out_args: vec![],
        quiet: true,
    })
    .unwrap();
    assert_eq!((summary.width, summary.height), (80, 60));
    assert_eq!(summary.frames, 60);
    let (frames, audio_streams) = probe_streams(&out);
    assert_eq!(frames, 60);
    assert_eq!(audio_streams, 1);
}

#[test]
fn trimmed_render_without_audio() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (clip, song) = synth(dir.path());
    let out = dir.path().join("out.mp4");
    let summary = render(&RenderOptions {
        video: clip,
        audio: song,
        map: map(),
        output: out.clone(),
        preview: None,
        start_s: Some(0.5),
        duration_s: Some(1.0),
        audio_mode: AudioMode::None,
        extra_out_args: vec![],
        quiet: true,
    })
    .unwrap();
    assert_eq!(summary.frames, 30);
    let (frames, audio_streams) = probe_streams(&out);
    assert_eq!(frames, 30);
    assert_eq!(audio_streams, 0);
}

#[test]
fn render_rejects_invalid_timeline_before_invoking_tools() {
    let base = RenderOptions {
        video: "/missing/video.mp4".into(),
        audio: "/missing/audio.wav".into(),
        map: "/missing/map.toml".into(),
        output: "/missing/out.mp4".into(),
        preview: None,
        start_s: None,
        duration_s: None,
        audio_mode: AudioMode::None,
        extra_out_args: vec![],
        quiet: true,
    };
    for preview in [Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let error = render(&RenderOptions {
            preview,
            ..base.clone()
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("--preview"));
    }
    for start_s in [Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let error = render(&RenderOptions {
            start_s,
            ..base.clone()
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("--start"));
    }
    for duration_s in [Some(-1.0), Some(0.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let error = render(&RenderOptions {
            duration_s,
            ..base.clone()
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("--duration"));
    }
}

#[test]
fn render_rejects_an_output_that_aliases_an_input() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (clip, song) = synth(dir.path());
    let original = std::fs::read(&clip).unwrap();
    let error = render(&RenderOptions {
        video: clip.clone(),
        audio: song,
        map: map(),
        output: clip.clone(),
        preview: None,
        start_s: None,
        duration_s: None,
        audio_mode: AudioMode::None,
        extra_out_args: vec![],
        quiet: true,
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("aliases video input"));
    assert_eq!(std::fs::read(clip).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn render_rejects_hardlink_outputs_without_changing_inputs() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    for input_kind in ["video", "audio", "mapping"] {
        let dir = tempfile::tempdir().unwrap();
        let (clip, song) = synth(dir.path());
        let mapping = dir.path().join("map.toml");
        std::fs::copy(map(), &mapping).unwrap();
        let input = match input_kind {
            "video" => &clip,
            "audio" => &song,
            "mapping" => &mapping,
            _ => unreachable!(),
        };
        let output = dir.path().join("out.mp4");
        std::fs::hard_link(input, &output).unwrap();
        let original = std::fs::read(input).unwrap();
        let error = render(&RenderOptions {
            video: clip.clone(),
            audio: song.clone(),
            map: mapping.clone(),
            output,
            preview: None,
            start_s: None,
            duration_s: None,
            audio_mode: AudioMode::None,
            extra_out_args: vec![],
            quiet: true,
        })
        .unwrap_err();
        assert!(
            format!("{error:#}").contains(&format!("aliases {input_kind} input")),
            "{error:#}"
        );
        assert_eq!(std::fs::read(input).unwrap(), original, "{input_kind}");
    }
}

#[test]
fn encoder_failure_returns_promptly_and_reports_ffmpeg_diagnostic() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("long.mp4");
    let song = dir.path().join("song.wav");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x120:rate=30:duration=20",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=20"])
            .arg(&song)
            .status()
            .unwrap()
            .success()
    );
    let started = Instant::now();
    let error = render(&RenderOptions {
        video: clip,
        audio: song,
        map: map(),
        output: dir.path().join("out.mp4"),
        preview: None,
        start_s: None,
        duration_s: None,
        audio_mode: AudioMode::None,
        extra_out_args: vec!["-definitely-invalid-option".into()],
        quiet: true,
    })
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        format!("{error:#}").contains("definitely-invalid-option"),
        "{error:#}"
    );
}

#[test]
fn analyze_preserves_the_resolved_preset_input() {
    if !tool_available("ffmpeg") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_, song) = synth(dir.path());
    let config = dir.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let preset = config.join("preset.json");
    let original = br#"{"version":1,"snow_intensity":42}"#;
    std::fs::write(&preset, original).unwrap();
    let mapping = config.join("map.toml");
    std::fs::write(&mapping, "[base]\npreset = \"preset.json\"\n").unwrap();
    let result = reactive_vcr::analyze::analyze(&reactive_vcr::analyze::AnalyzeOptions {
        audio: song,
        map: mapping,
        fps: 30.0,
        output: preset.clone(),
        start_s: None,
        duration_s: None,
    });
    assert!(
        std::fs::read(&preset).unwrap() == original,
        "preset was overwritten"
    );
    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("aliases preset input"),
        "{error:#}"
    );
}

#[cfg(unix)]
#[test]
fn render_preserves_preset_through_canonical_and_hardlink_aliases() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (clip, song) = synth(dir.path());
    let config = dir.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let preset = config.join("preset.json");
    let original = br#"{"version":1,"snow_intensity":42}"#;
    let mapping = config.join("map.toml");
    std::fs::write(&mapping, "[base]\npreset = \"preset.json\"\n").unwrap();
    std::fs::write(&preset, original).unwrap();
    let symbolic = dir.path().join("symbolic.mp4");
    std::os::unix::fs::symlink(&preset, &symbolic).unwrap();
    let hardlink = dir.path().join("hardlink.mp4");
    std::fs::hard_link(&preset, &hardlink).unwrap();
    for output in [
        preset.clone(),
        config.join("./preset.json"),
        symbolic,
        hardlink,
    ] {
        let result = render(&RenderOptions {
            video: clip.clone(),
            audio: song.clone(),
            map: mapping.clone(),
            output,
            preview: None,
            start_s: None,
            duration_s: None,
            audio_mode: AudioMode::None,
            extra_out_args: vec!["-f".into(), "mp4".into()],
            quiet: true,
        });
        assert!(
            std::fs::read(&preset).unwrap() == original,
            "preset was overwritten"
        );
        let error = result.unwrap_err();
        assert!(
            format!("{error:#}").contains("aliases preset input"),
            "{error:#}"
        );
    }
}

#[test]
fn variable_rate_video_uses_the_selected_constant_frame_timeline() {
    if !tool_available("ffmpeg") || !tool_available("ffprobe") {
        eprintln!("skip: ffmpeg missing");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_, song) = synth(dir.path());
    let clip = dir.path().join("variable.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=30:duration=2",
            "-vf",
            "select='if(lt(t,1),1,not(mod(n,3)))'",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&clip)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let input = reactive_vcr::probe::probe(&clip).unwrap();
    assert!(
        (input.fps - 30.0).abs() > 1.0,
        "fixture must be variable rate"
    );
    let mapping = dir.path().join("empty.toml");
    std::fs::write(&mapping, "").unwrap();
    for (start_s, duration_s) in [(None, None), (Some(0.45), Some(0.9))] {
        let output = dir.path().join("out.mp4");
        let summary = render(&RenderOptions {
            video: clip.clone(),
            audio: song.clone(),
            map: mapping.clone(),
            output: output.clone(),
            preview: None,
            start_s,
            duration_s,
            audio_mode: AudioMode::None,
            extra_out_args: vec![],
            quiet: true,
        })
        .unwrap();
        let actual = reactive_vcr::probe::probe(&output).unwrap();
        let expected_duration = duration_s.unwrap_or(input.duration_s);
        let frame_duration = 1.0 / input.fps;
        assert!(
            (actual.duration_s - expected_duration).abs() <= frame_duration,
            "expected {expected_duration}s, got {}s at {} fps ({} frames)",
            actual.duration_s,
            actual.fps,
            summary.frames
        );
        assert!((actual.fps - input.fps).abs() < 1e-5);
        assert!((summary.frames as f64 - expected_duration * input.fps).abs() <= 1.0);
        let timestamps = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "json",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(timestamps.status.success());
        let frames: serde_json::Value = serde_json::from_slice(&timestamps.stdout).unwrap();
        let times: Vec<f64> = frames["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| {
                frame["best_effort_timestamp_time"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(times.len(), summary.frames);
        assert!(
            times
                .windows(2)
                .all(|pair| (pair[1] - pair[0] - frame_duration).abs() < 1e-5)
        );
    }
}
