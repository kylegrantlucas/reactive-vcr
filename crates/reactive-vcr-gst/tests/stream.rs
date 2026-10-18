mod common;

use common::example;
use reactive_vcr_gst::stream::{StreamOptions, stream};

const FRAME_BYTES: usize = 160 * 120 * 3;

fn stream_frames(dir: &std::path::Path, name: &str, audio_source: &str) -> Vec<Vec<u8>> {
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
