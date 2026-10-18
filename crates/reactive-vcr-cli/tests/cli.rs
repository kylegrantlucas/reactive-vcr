use reactive_vcr_gst::gst;
use reactive_vcr_gst::gst::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

const WIDTH: usize = 160;
const HEIGHT: usize = 120;
/// One frame as FFV1 output stores it: 4:4:4 YUV, three bytes per pixel.
const FRAME_BYTES: usize = WIDTH * HEIGHT * 3;
/// The format and colorimetry render's FFV1 output uses at this size.
const RENDERED_CAPS: &str = "video/x-raw,format=Y444,colorimetry=bt601";

fn run_pipeline(description: &str) {
    reactive_vcr_gst::init().unwrap();
    let pipeline = gst::parse::launch(description)
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
    // gst-libav 1.24's frame-threaded FFV1 decoder sometimes loses the last frame at end of
    // stream, which would make frame counts flaky; decode it on one thread instead.
    pipeline.connect_deep_element_added(|_, _, element| {
        if element
            .factory()
            .is_some_and(|factory| factory.name() == "avdec_ffv1")
        {
            element.set_property("max-threads", 1i32);
        }
    });
    pipeline.set_state(gst::State::Playing).unwrap();
    let message = pipeline
        .bus()
        .unwrap()
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(60),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .expect("pipeline finished");
    pipeline.set_state(gst::State::Null).unwrap();
    if let gst::MessageView::Error(error) = message.view() {
        panic!("{description}: {}", error.error());
    }
}

/// Lossless test pattern, so decoded source pixels are identical everywhere.
fn clip(dir: &Path) -> PathBuf {
    let path = dir.join("clip.mkv");
    run_pipeline(&format!(
        "videotestsrc pattern=smpte num-buffers=90 ! video/x-raw,width={WIDTH},height={HEIGHT},framerate=30/1 \
         ! videoconvert ! video/x-raw,format=BGRx ! avenc_ffv1 ! matroskamux ! filesink location={}",
        path.display()
    ));
    path
}

fn song(dir: &Path) -> PathBuf {
    let path = dir.join("song.wav");
    run_pipeline(&format!(
        "audiotestsrc freq=110 samplesperbuffer=4800 num-buffers=30 ! audio/x-raw,rate=48000 \
         ! audioconvert ! wavenc ! filesink location={}",
        path.display()
    ));
    path
}

fn binary(args: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_reactive-vcr"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "reactive-vcr {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn frames(raw: Vec<u8>) -> Vec<[u8; FRAME_BYTES]> {
    let (frames, rest) = raw.as_chunks::<FRAME_BYTES>();
    assert!(rest.is_empty(), "partial frame");
    frames.to_vec()
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// Play and render must apply identical effects to identical source pixels, so on a lossless
/// source every played frame, converted to YUV as render's FFV1 output is, is byte-identical
/// to the rendered one.
#[test]
fn played_frames_match_rendered_frames() {
    let dir = tempfile::tempdir().unwrap();
    let (clip, song) = (clip(dir.path()), song(dir.path()));
    let rendered = dir.path().join("rendered.mkv");
    let rendered_yuv = dir.path().join("rendered.yuv");
    let played_yuv = dir.path().join("played.yuv");
    // Feedback blends every frame into the next, so state left over from the preroll frame
    // before the initial seek would show up in every played frame.
    let feedback = dir.path().join("feedback.toml");
    std::fs::write(
        &feedback,
        r#"
[bender.set]
"feedback.enabled" = true
"feedback.amount" = 0.8

[[modulate]]
target = "bender.feedback.zoom"
source = "rms"
range = [1.0, 1.1]
"#,
    )
    .unwrap();

    for map in [example("basic.toml"), feedback] {
        let common = [
            "--video",
            clip.to_str().unwrap(),
            "--audio",
            song.to_str().unwrap(),
            "--map",
            map.to_str().unwrap(),
            "--start",
            "0.5",
            "--duration",
            "1",
            "--quiet",
        ];
        binary(
            &[
                &["render"][..],
                &common,
                &["-o", rendered.to_str().unwrap()],
            ]
            .concat(),
        );
        run_pipeline(&format!(
            "filesrc location={} ! decodebin ! videoconvert ! {RENDERED_CAPS} \
             ! filesink location={}",
            rendered.display(),
            rendered_yuv.display()
        ));
        let video_sink = format!(
            "videoconvert ! {RENDERED_CAPS} ! filesink location={}",
            played_yuv.display()
        );
        binary(
            &[
                &["play"][..],
                &common,
                &["--audio-sink", "fakesink", "--video-sink", &video_sink],
            ]
            .concat(),
        );

        let rendered = frames(std::fs::read(&rendered_yuv).unwrap());
        let played = frames(std::fs::read(&played_yuv).unwrap());
        assert_eq!(rendered.len(), 30, "{}", map.display());
        // The sink also receives the preroll frame shown before the initial seek.
        assert!(played.len() >= rendered.len(), "{}", map.display());
        let tail = &played[played.len() - rendered.len()..];
        for (index, (played, rendered)) in tail.iter().zip(&rendered).enumerate() {
            assert!(
                played == rendered,
                "{} frame {index} differs from render",
                map.display()
            );
        }
    }
}

#[test]
fn analyze_writes_one_csv_row_per_frame() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("features.csv");
    let output_arg = output.to_str().unwrap();
    binary(&[
        "analyze",
        "--audio",
        song(dir.path()).to_str().unwrap(),
        "--map",
        example("basic.toml").to_str().unwrap(),
        "--fps",
        "4",
        "--duration",
        "0.5",
        "-o",
        output_arg,
    ]);
    let lines: Vec<_> = std::fs::read_to_string(output)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0],
        "frame,time,rms,band_bass,band_high,onset,tracking_noise,tracking_noise_wave_intensity,tracking_noise_height,head_switching,head_switching_horizontal_shift,snow_intensity,chroma_noise_intensity"
    );
    assert!(lines[1].starts_with("0,0.1250,"));
    assert!(lines[2].starts_with("1,0.3750,"));
}

#[test]
fn render_needs_a_known_output_extension() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_reactive-vcr"))
        .args(["render", "--video", "in.mp4", "--audio", "a.wav", "--map"])
        .arg(example("basic.toml"))
        .args(["-o"])
        .arg(dir.path().join("out.avi"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pass --codec"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
