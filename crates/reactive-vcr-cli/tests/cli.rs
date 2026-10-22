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

/// Run `reactive-vcr check` with `args`; returns its exit code, stdout, and stderr.
fn check(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_reactive-vcr"))
        .arg("check")
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code().expect("exit code"),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

const TWO_PROBLEMS: &str = r#"
[[modulate]]
target = "snow_intensty"
source = "rms"

[[modulate]]
target = "snow_intensity"
source = "band:nope"
"#;

#[test]
fn check_accepts_a_valid_mapping() {
    let map = example("basic.toml");
    let (code, stdout, _) = check(&["--map", map.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("ok"), "{stdout}");

    let (code, stdout, _) = check(&["--map", map.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "{\"ok\":true}\n");
}

#[test]
fn check_reports_each_problem_of_an_invalid_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("bad.toml");
    std::fs::write(&map, TWO_PROBLEMS).unwrap();

    let (code, _, stderr) = check(&["--map", map.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("snow_intensty") && stderr.contains("nope"),
        "{stderr}"
    );

    let (code, stdout, _) = check(&["--map", map.to_str().unwrap(), "--json"]);
    assert_eq!(code, 1);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["ok"], false);
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{report}");
    assert!(errors[0].as_str().unwrap().contains("snow_intensty"));
    assert!(errors[1].as_str().unwrap().contains("nope"));
}

#[test]
fn check_reports_syntax_and_preset_errors_as_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let syntax = dir.path().join("syntax.toml");
    std::fs::write(&syntax, "[[modulate]\n").unwrap();
    let (code, stdout, _) = check(&["--map", syntax.to_str().unwrap(), "--json"]);
    assert_eq!(code, 1);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{report}");
    assert!(errors[0].as_str().unwrap().contains("parsing mapping"));

    // Presets resolve from the mapping's directory, not the working directory.
    let nested = dir.path().join("maps");
    std::fs::create_dir(&nested).unwrap();
    let preset = nested.join("preset.toml");
    std::fs::write(&preset, "[bender]\npreset = \"missing.json\"\n").unwrap();
    let (code, stdout, _) = check(&["--map", preset.to_str().unwrap(), "--json"]);
    assert_eq!(code, 1);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let error = report["errors"][0].as_str().unwrap();
    assert!(
        error.contains(&nested.join("missing.json").display().to_string()),
        "{error}"
    );
}

#[test]
fn check_exits_2_when_the_mapping_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let (code, stdout, _) = check(&["--map", missing.to_str().unwrap(), "--json"]);
    assert_eq!(code, 2);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["ok"], false);
    assert!(
        report["errors"][0]
            .as_str()
            .unwrap()
            .contains("reading mapping"),
        "{report}"
    );

    let (code, _, _) = check(&[]);
    assert_eq!(code, 2, "a missing --map is a usage error");
}

/// Run `stream` with telemetry to a local socket, calling `on_datagram` for each message as it
/// arrives; returns every message and the process output.
fn stream_telemetry(
    args: &[&str],
    mut on_datagram: impl FnMut(&serde_json::Value),
) -> (Vec<serde_json::Value>, std::process::Output) {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .unwrap();
    let address = socket.local_addr().unwrap().to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_reactive-vcr"))
        .arg("stream")
        .args(args)
        .args([
            "--telemetry",
            &address,
            "--telemetry-id",
            "cam-1",
            "--quiet",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut messages = Vec::new();
    let mut buffer = [0u8; 65_536];
    let mut exited = false;
    loop {
        match socket.recv(&mut buffer) {
            Ok(len) => {
                assert!(len < 8192, "{len}-byte datagram");
                let message: serde_json::Value = serde_json::from_slice(&buffer[..len]).unwrap();
                on_datagram(&message);
                messages.push(message);
            }
            // Drain once more after the process exits, then stop.
            Err(_) if exited => break,
            Err(_) => exited = child.try_wait().unwrap().is_some(),
        }
    }
    (messages, child.wait_with_output().unwrap())
}

#[test]
fn stream_sends_start_and_frame_telemetry() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.toml");
    std::fs::copy(example("basic.toml"), &map).unwrap();
    let map_arg = map.to_str().unwrap().to_owned();
    let mut replaced = false;
    let (messages, output) = stream_telemetry(
        &[
            "--map",
            &map_arg,
            "--video-source",
            "videotestsrc is-live=true num-buffers=90 \
             ! video/x-raw,width=160,height=120,framerate=30/1",
            "--audio-source",
            "audiotestsrc is-live=true wave=sine freq=110",
            "--video-sink",
            "fakesink",
            "--lines",
            "120",
            "--telemetry-hz",
            "10",
        ],
        |message| {
            // Install a broken mapping the way a control server would: write, then rename.
            if message["kind"] == "frame" && !replaced {
                replaced = true;
                let temp = map.with_extension("tmp");
                std::fs::write(&temp, "[[modulate]]\ntarget = \"nope\"\nsource = \"rms\"\n")
                    .unwrap();
                std::fs::rename(&temp, &map).unwrap();
            }
        },
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let start = &messages[0];
    assert_eq!(start["v"], 1);
    assert_eq!(start["id"], "cam-1");
    assert_eq!(start["kind"], "start");
    assert_eq!(start["map"], map_arg.as_str());
    assert_eq!(start["lines"], 120);
    assert!(start["ts"].as_u64().unwrap() > 1_700_000_000_000);

    let frames: Vec<_> = messages.iter().filter(|m| m["kind"] == "frame").collect();
    assert!(frames.len() >= 5, "{} frame messages", frames.len());
    let targets = [
        "tracking_noise",
        "tracking_noise_wave_intensity",
        "tracking_noise_height",
        "head_switching",
        "head_switching_horizontal_shift",
        "snow_intensity",
        "chroma_noise_intensity",
    ];
    for frame in &frames {
        assert_eq!(frame["id"], "cam-1");
        assert_eq!(frame["v"], 1);
        assert!(frame["frames"].as_u64().unwrap() >= 1, "{frame}");
        assert!(frame["dropped"].as_u64().is_some(), "{frame}");
        assert!(frame["fps"].as_f64().unwrap() > 0.0, "{frame}");
        assert!(frame["proc_ms"].as_f64().unwrap() > 0.0, "{frame}");
        let features = &frame["features"];
        for value in [
            &features["rms"],
            &features["onset"],
            &features["bands"]["bass"],
            &features["bands"]["high"],
        ] {
            let value = value.as_f64().unwrap_or_else(|| panic!("{frame}"));
            assert!((0.0..=1.0).contains(&value), "{frame}");
        }
        let applied = frame["targets"].as_object().unwrap();
        assert_eq!(applied.len(), targets.len(), "{frame}");
        for target in targets {
            assert!(applied[target].is_number(), "{target}: {frame}");
        }
    }
    // The 110 Hz tone fills the bass band.
    assert!(
        frames
            .iter()
            .any(|frame| frame["features"]["bands"]["bass"].as_f64().unwrap() > 0.5)
    );

    // At most 10 messages a second: any one-second span holds at most 11 (10 intervals).
    let stamps: Vec<u64> = frames.iter().map(|f| f["ts"].as_u64().unwrap()).collect();
    for (index, &first) in stamps.iter().enumerate() {
        let within = stamps[index..].iter().take_while(|&&ts| ts < first + 1000);
        assert!(within.count() <= 11, "frame messages at {stamps:?}");
    }

    // The broken mapping is reported while the last good one stays in effect.
    assert!(frames[0]["mapping_error"].is_null());
    let reported = frames
        .iter()
        .rev()
        .find_map(|frame| frame["mapping_error"].as_str())
        .expect("a frame reporting the reload error");
    assert!(reported.contains("nope"), "{reported}");
    assert_eq!(
        frames.last().unwrap()["targets"].as_object().unwrap().len(),
        7
    );
}

#[test]
fn stream_reports_a_fatal_error_over_telemetry() {
    let (messages, output) = stream_telemetry(
        &[
            "--map",
            example("basic.toml").to_str().unwrap(),
            "--video-source",
            "videotestsrc is-live=true num-buffers=30",
            "--audio-source",
            "nonexistentaudiosrc",
            "--video-sink",
            "fakesink",
        ],
        |_| {},
    );
    assert!(!output.status.success());
    let error = messages
        .iter()
        .find(|m| m["kind"] == "error")
        .expect("an error message");
    assert_eq!(error["id"], "cam-1");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("nonexistentaudiosrc"), "{message}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for part in message.split(": ") {
        assert!(stderr.contains(part), "{part:?} missing from {stderr}");
    }
}
