mod common;

use common::{clip, example, run};
use reactive_vcr_gst::stream::{StreamOptions, StreamSummary, stream};
use std::path::Path;
use std::time::{Duration, Instant};

const FRAME_BYTES: usize = 160 * 120 * 3;
const LIVE_AUDIO: &str = "audiotestsrc is-live=true wave=silence";

fn stream_with(video_source: &str, video_sink: &str) -> anyhow::Result<StreamSummary> {
    stream(&StreamOptions {
        video_threads: None,
        encode_queue: false,
        map: example("basic.toml"),
        video_source: video_source.into(),
        audio_source: LIVE_AUDIO.into(),
        lines: 120,
        video_sink: video_sink.into(),
        audio_sink: "fakesink".into(),
        gate_db: None,
        telemetry: None,
        quiet: true,
    })
}

fn stream_frames(dir: &Path, name: &str, audio_source: &str) -> Vec<Vec<u8>> {
    let output = dir.join(name);
    let options = StreamOptions {
        video_threads: None,
        encode_queue: false,
        map: example("basic.toml"),
        video_source: "videotestsrc is-live=true pattern=smpte num-buffers=30 \
                   ! video/x-raw,width=160,height=120,framerate=30/1"
            .into(),
        audio_source: audio_source.into(),
        lines: 120,
        video_sink: format!(
            "videoconvert ! video/x-raw,format=RGB ! filesink location={}",
            output.display()
        ),
        audio_sink: "fakesink".into(),
        gate_db: None,
        telemetry: None,
        quiet: true,
    };
    let summary = stream(&options).unwrap();
    let bytes = std::fs::read(&output).unwrap();
    let (frames, _) = bytes.as_chunks::<FRAME_BYTES>();
    let frames: Vec<Vec<u8>> = frames.iter().map(|frame| frame.to_vec()).collect();
    assert!(!frames.is_empty(), "{audio_source}: no whole frames");
    assert!(summary.frames >= frames.len() as u64);
    frames
}

#[test]
fn live_audio_drives_the_effect() {
    let dir = tempfile::tempdir().unwrap();
    let silent = stream_frames(
        dir.path(),
        "silent.rgb",
        "audiotestsrc is-live=true wave=silence",
    );
    let tone = stream_frames(
        dir.path(),
        "tone.rgb",
        "audiotestsrc is-live=true wave=sine freq=110 volume=1.0",
    );
    let pairs = silent.len().min(tone.len());
    let differing = silent
        .iter()
        .zip(&tone)
        .filter(|(silent, tone)| silent != tone)
        .count();
    assert!(
        differing * 2 >= pairs,
        "only {differing} of {pairs} frames differ between silence and a tone"
    );
}

#[test]
fn explicit_audio_sink_receives_the_driving_audio() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("monitor.f32");
    let summary = stream(&StreamOptions {
        video_threads: None,
        encode_queue: false,
        map: example("basic.toml"),
        video_source: "videotestsrc num-buffers=30 \
                   ! video/x-raw,width=160,height=120,framerate=30/1"
            .into(),
        audio_source: "audiotestsrc wave=sine freq=110 volume=1.0 \
                   samplesperbuffer=480 num-buffers=10 \
                   ! audio/x-raw,format=F32LE,channels=1,rate=48000"
            .into(),
        lines: 120,
        video_sink: "fakesink sync=true".into(),
        audio_sink: format!(
            "audio/x-raw,format=F32LE,channels=1,rate=48000 ! filesink location={}",
            output.display()
        ),
        gate_db: None,
        telemetry: None,
        quiet: true,
    })
    .unwrap();
    assert_eq!(summary.frames, 30);
    let bytes = std::fs::read(output).unwrap();
    assert_eq!(bytes.len(), 480 * 10 * 4);
    assert!(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .any(|sample| f32::from_le_bytes(*sample).abs() > 0.5),
        "the explicit monitor sink should receive the tone"
    );
}

#[test]
fn file_sources_with_demuxers_and_decoders_stream() {
    let dir = tempfile::tempdir().unwrap();
    // 30 frames with an audio track, so a decoder also offers a stream the video can't take.
    let clip = clip(dir.path(), 1, true);
    let uri = reactive_vcr_gst::probe::file_uri(&clip).unwrap();
    for source in [
        format!("filesrc location={} ! decodebin", clip.display()),
        format!("uridecodebin uri={uri}"),
        format!(
            "filesrc location={} ! matroskademux ! avdec_ffv1",
            clip.display()
        ),
    ] {
        let summary = stream_with(&source, "fakesink sync=true")
            .unwrap_or_else(|error| panic!("{source}: {error:#}"));
        // gst-libav's frame-threaded FFV1 decoder can lose the last frame at end of stream.
        assert!(summary.frames >= 29, "{source}: {} frames", summary.frames);
    }
}

#[test]
fn non_live_sources_play_every_frame_at_their_own_rate() {
    let started = Instant::now();
    let summary = stream_with(
        "videotestsrc num-buffers=60 ! video/x-raw,width=160,height=120,framerate=60/1",
        "fakesink sync=true",
    )
    .unwrap();
    assert_eq!(summary.frames, 60);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900),
        "60 frames at 60 fps took {elapsed:?}"
    );
}

#[test]
fn live_sources_that_outpace_the_effect_drop_stale_frames() {
    // The sink takes 100 ms per frame, so a 30 fps camera outpaces it threefold.
    let summary = stream_with(
        "videotestsrc is-live=true num-buffers=30 \
         ! video/x-raw,width=160,height=120,framerate=30/1",
        "identity sleep-time=100000 ! fakesink sync=false",
    )
    .unwrap();
    assert!(
        summary.frames > 0 && summary.frames <= 20,
        "{} of 30 frames processed",
        summary.frames
    );
}

#[cfg(unix)]
#[test]
fn piped_sources_that_start_late_play_in_real_time() {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;

    const FRAMES: usize = 60;
    let dir = tempfile::tempdir().unwrap();
    let y4m = dir.path().join("clip.y4m");
    run(&format!(
        "videotestsrc pattern=smpte num-buffers={FRAMES} \
         ! video/x-raw,format=I420,width=160,height=120,framerate=30/1 \
         ! y4menc ! filesink location={}",
        y4m.display()
    ));
    let data = std::fs::read(&y4m).unwrap();
    let (reader, mut writer) = std::io::pipe().unwrap();
    // The writer starts a second after the clip would have ended, so the pipe's timestamps,
    // which start at zero, put every frame over a second late against the running time.
    let clip = Duration::from_secs_f64(FRAMES as f64 / 30.0);
    let delay = clip + Duration::from_secs(1);
    let feeder = std::thread::spawn(move || {
        std::thread::sleep(delay);
        writer.write_all(&data).unwrap();
    });
    let output = dir.path().join("out.rgb");
    let started = Instant::now();
    // A sink that discards frames more than 200 ms late, like a display sink.
    stream_with(
        &format!("fdsrc fd={} ! y4mdec", reader.as_raw_fd()),
        &format!(
            "videoconvert ! video/x-raw,format=RGB \
             ! filesink sync=true qos=true max-lateness=200000000 location={}",
            output.display()
        ),
    )
    .unwrap();
    let elapsed = started.elapsed();
    feeder.join().unwrap();
    drop(reader);
    let shown = std::fs::read(&output).unwrap().len() / FRAME_BYTES;

    // Played as late frames, the clip is dropped and over as soon as it arrives. Played in
    // real time from its arrival, it lasts as long as the clip and most frames are shown;
    // not all, because a frame that reaches the sink late, such as the first while the
    // effect starts up or one that a busy machine processes slowly, makes the converters skip
    // the next ones to catch up, as they would for a camera.
    let real_time = delay + clip.mul_f64(0.9);
    assert!(
        elapsed >= real_time,
        "{shown} of {FRAMES} frames shown within {elapsed:?}"
    );
    assert!(
        shown * 2 >= FRAMES,
        "{shown} of {FRAMES} frames shown within {elapsed:?}"
    );
}

#[test]
fn sinks_can_start_with_caps() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.i420");
    let summary = stream_with(
        "videotestsrc is-live=true num-buffers=10 \
         ! video/x-raw,width=160,height=120,framerate=30/1",
        &format!(
            "video/x-raw,format=I420 ! filesink location={}",
            output.display()
        ),
    )
    .unwrap();
    let bytes = std::fs::read(&output).unwrap().len();
    assert!(summary.frames > 0);
    assert_eq!(bytes as u64, summary.frames * (160 * 120 * 3 / 2));
}

#[test]
fn encoded_output_drains_after_video_ends_with_live_audio() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("complete.mkv");
    let summary = stream_with(
        "videotestsrc num-buffers=32 \
         ! video/x-raw,width=160,height=120,framerate=30/1",
        &format!(
            "queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 \
             ! identity sleep-time=50000 ! videoconvert ! video/x-raw,format=I420 \
             ! x264enc tune=zerolatency speed-preset=ultrafast \
             ! h264parse ! matroskamux ! filesink location={}",
            output.display()
        ),
    )
    .unwrap();
    assert_eq!(summary.frames, 32);
    let decoded = dir.path().join("decoded.rgb");
    run(&format!(
        "filesrc location={} ! matroskademux ! h264parse ! avdec_h264 max-threads=1 \
         ! videoconvert ! video/x-raw,format=RGB ! filesink location={}",
        output.display(),
        decoded.display()
    ));
    assert_eq!(
        std::fs::read(decoded).unwrap().len(),
        32 * FRAME_BYTES,
        "the encoder and muxer must drain every decoded frame"
    );
}

#[test]
fn converter_workers_and_encoder_queue_preserve_ordered_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut reference = None;
    for (index, (video_threads, encode_queue)) in [
        (None, false),
        (Some(1), false),
        (Some(2), false),
        (Some(1), true),
        (Some(2), true),
    ]
    .into_iter()
    .enumerate()
    {
        let output = dir.path().join(format!("frames-{index}.i420"));
        let result = stream(&StreamOptions {
            map: example("basic.toml"),
            video_source: "videotestsrc pattern=ball num-buffers=12 \
                ! video/x-raw,format=I420,width=32,height=24,framerate=12/1"
                .into(),
            audio_source: LIVE_AUDIO.into(),
            lines: 24,
            video_threads,
            encode_queue,
            video_sink: format!(
                "video/x-raw,format=I420 ! filesink location={}",
                output.display()
            ),
            audio_sink: "fakesink".into(),
            gate_db: Some(-60.0),
            telemetry: None,
            quiet: true,
        })
        .unwrap();
        assert_eq!(result.frames, 12, "no input frame may be lost");
        let bytes = std::fs::read(output).unwrap();
        assert_eq!(
            bytes.len(),
            12 * 32 * 24 * 3 / 2,
            "all encoded raw frames must drain"
        );
        if let Some(expected) = &reference {
            assert_eq!(
                &bytes, expected,
                "workers={video_threads:?}, queue={encode_queue}"
            );
        } else {
            reference = Some(bytes);
        }
    }
}

#[test]
fn processing_height_scales_the_raster_without_changing_display_aspect() {
    let dir = tempfile::tempdir().unwrap();
    for (name, input_width, height, par, lines, ideal_width) in [
        ("ntsc", 720, 480, "1/1", 360, 540.0),
        ("anamorphic", 720, 480, "8/9", 360, 480.0),
        ("pal", 720, 576, "1/1", 360, 450.0),
        ("hd", 1920, 1080, "1/1", 480, 1920.0 * 480.0 / 1080.0),
    ] {
        let output = dir.path().join(format!("{name}.y4m"));
        let summary = stream(&StreamOptions {
            map: example("basic.toml"),
            video_source: format!(
                "videotestsrc num-buffers=1 ! video/x-raw,width={input_width},height={height},\
                 pixel-aspect-ratio={par},framerate=30/1"
            ),
            audio_source: LIVE_AUDIO.into(),
            lines,
            video_threads: Some(1),
            encode_queue: false,
            video_sink: format!(
                "video/x-raw,format=I420 ! y4menc ! filesink location={}",
                output.display()
            ),
            audio_sink: "fakesink".into(),
            gate_db: Some(-60.0),
            telemetry: None,
            quiet: true,
        })
        .unwrap();
        assert_eq!(summary.frames, 1);
        let (info, _) = common::decoded(&output, "identity");
        assert_eq!(info.height(), lines, "{name}");
        assert_eq!(info.width() % 2, 0, "4:2:0 encoder width: {name}");
        // An even raster quantizes the ideal width by less than one two-pixel step.
        assert!(
            (f64::from(info.width()) - ideal_width).abs() < 2.0,
            "{name}"
        );
        assert_eq!(
            info.par(),
            reactive_vcr_gst::gst::Fraction::new(1, 1),
            "{name}"
        );
    }
}
