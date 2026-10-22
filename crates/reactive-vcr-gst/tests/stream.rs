mod common;

use common::{clip, example, run};
use reactive_vcr_gst::stream::{StreamOptions, StreamSummary, stream};
use std::path::Path;
use std::time::{Duration, Instant};

const FRAME_BYTES: usize = 160 * 120 * 3;
const LIVE_AUDIO: &str = "audiotestsrc is-live=true wave=silence";

fn stream_with(video_source: &str, video_sink: &str) -> anyhow::Result<StreamSummary> {
    stream(&StreamOptions {
        map: example("basic.toml"),
        video_source: video_source.into(),
        audio_source: LIVE_AUDIO.into(),
        lines: 120,
        video_sink: video_sink.into(),
        audio_sink: "fakesink".into(),
        telemetry: None,
        quiet: true,
    })
}

fn stream_frames(dir: &Path, name: &str, audio_source: &str) -> Vec<Vec<u8>> {
    let output = dir.join(name);
    let options = StreamOptions {
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

    let dir = tempfile::tempdir().unwrap();
    let y4m = dir.path().join("clip.y4m");
    run(&format!(
        "videotestsrc pattern=smpte num-buffers=30 \
         ! video/x-raw,format=I420,width=160,height=120,framerate=30/1 \
         ! y4menc ! filesink location={}",
        y4m.display()
    ));
    let data = std::fs::read(&y4m).unwrap();
    let (reader, mut writer) = std::io::pipe().unwrap();
    // The writer starts a second after the stream, so the pipe's timestamps, which start
    // at zero, are a second late against the running time.
    let feeder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        writer.write_all(&data).unwrap();
    });
    let output = dir.path().join("out.rgb");
    // A sink that discards frames more than 200 ms late, like a display sink.
    let summary = stream_with(
        &format!("fdsrc fd={} ! y4mdec", reader.as_raw_fd()),
        &format!(
            "videoconvert ! video/x-raw,format=RGB \
             ! filesink sync=true qos=true max-lateness=200000000 location={}",
            output.display()
        ),
    )
    .unwrap();
    feeder.join().unwrap();
    drop(reader);
    let shown = std::fs::read(&output).unwrap().len() / FRAME_BYTES;
    assert_eq!(summary.frames, 30);
    assert!(shown >= 29, "{shown} of 30 frames shown");
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
