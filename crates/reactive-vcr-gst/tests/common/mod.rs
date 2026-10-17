//! Synthetic media and frame readers for integration tests, built with GStreamer itself.
#![allow(dead_code)]

use reactive_vcr::mapping;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::{Selection, select_timeline, selection_features};
use reactive_vcr_gst::gst;
use reactive_vcr_gst::gst::prelude::*;
use reactive_vcr_gst::render::{OutputCodec, RenderJob, RenderSettings, RenderStatus};
use reactive_vcr_gst::{AudioMode, Program};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub fn run(description: &str) {
    reactive_vcr_gst::init().unwrap();
    let pipeline = gst::parse::launch(description).unwrap();
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
        panic!("{description}: {} ({:?})", error.error(), error.debug());
    }
}

/// A lossless FFV1 test-pattern clip with a 440 Hz FLAC track.
pub fn clip(dir: &Path, seconds: u32, with_audio: bool) -> PathBuf {
    let path = dir.join("clip.mkv");
    let frames = seconds * 30;
    let audio = if with_audio {
        format!(
            " audiotestsrc freq=440 samplesperbuffer=1600 num-buffers={frames} ! audio/x-raw,rate=48000 ! audioconvert ! flacenc ! m."
        )
    } else {
        String::new()
    };
    run(&format!(
        "videotestsrc pattern=smpte num-buffers={frames} ! video/x-raw,width=160,height=120,framerate=30/1 \
         ! videoconvert ! video/x-raw,format=BGRx ! avenc_ffv1 ! matroskamux name=m \
         ! filesink location={}{audio}",
        path.display()
    ));
    path
}

/// A pulsing 110 Hz tone as 44.1 kHz stereo WAV.
pub fn song(dir: &Path, seconds: u32) -> PathBuf {
    let path = dir.join("song.wav");
    let buffers = seconds * 10;
    run(&format!(
        "audiotestsrc freq=110 samplesperbuffer=4410 num-buffers={buffers} \
         ! audio/x-raw,rate=44100,channels=2 ! audioconvert ! wavenc ! filesink location={}",
        path.display()
    ));
    path
}

pub fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// Every video frame of `path` as packed RGB at its own size.
pub fn rgb_frames(path: &Path) -> Vec<Vec<u8>> {
    reactive_vcr_gst::init().unwrap();
    let pipeline = gst::parse::launch(&format!(
        "filesrc location={} ! decodebin ! videoconvert ! video/x-raw,format=RGB \
         ! appsink name=sink sync=false",
        path.display()
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    let sink = pipeline
        .by_name("sink")
        .unwrap()
        .downcast::<gstreamer_app::AppSink>()
        .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let mut frames = Vec::new();
    while let Ok(sample) = sink.pull_sample() {
        let caps = sample.caps().unwrap();
        let info = gstreamer_video::VideoInfo::from_caps(caps).unwrap();
        let map = sample.buffer().unwrap().map_readable().unwrap();
        let row = info.width() as usize * 3;
        let stride = info.stride()[0] as usize;
        frames.push(
            map.as_slice()
                .chunks(stride)
                .take(info.height() as usize)
                .flat_map(|line| &line[..row])
                .copied()
                .collect(),
        );
    }
    pipeline.set_state(gst::State::Null).unwrap();
    frames
}

/// Seconds of audio in the first audio stream, or `None` without one.
pub fn audio_seconds(path: &Path) -> Option<f64> {
    let info = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(30))
        .unwrap()
        .discover_uri(&reactive_vcr_gst::probe::file_uri(path).unwrap())
        .unwrap();
    info.audio_streams().first()?;
    let samples = reactive_vcr_gst::decode::decode_mono_f32(path).unwrap();
    Some(samples.len() as f64 / 48_000.0)
}

pub struct Render {
    pub video: PathBuf,
    pub audio: PathBuf,
    pub map: PathBuf,
    pub output: PathBuf,
    pub codec: OutputCodec,
    pub audio_mode: AudioMode,
    pub start: Option<f64>,
    pub duration: Option<f64>,
}

pub fn settings(r: &Render) -> anyhow::Result<RenderSettings> {
    let info = reactive_vcr_gst::probe::probe(&r.video)?;
    let selection: Selection = select_timeline(&info, None, r.start, r.duration)?;
    let catalog = Arc::new(ParamCatalog::new());
    let map = mapping::load(&r.map, &catalog)?;
    let samples = reactive_vcr_gst::decode::decode_mono_f32(&r.audio)?;
    let features = selection_features(&samples, &map, &selection)?;
    Ok(RenderSettings {
        video: r.video.clone(),
        audio: r.audio.clone(),
        output: r.output.clone(),
        codec: r.codec,
        audio_mode: r.audio_mode,
        selection,
        has_original_audio: info.has_audio,
        program: Arc::new(Program::new(catalog, &map, &features, &selection)),
    })
}

/// Render to completion; returns frames rendered.
pub fn render(r: &Render) -> anyhow::Result<u64> {
    let job = RenderJob::start(settings(r)?)?;
    loop {
        match job.status() {
            RenderStatus::Running { .. } => std::thread::sleep(Duration::from_millis(20)),
            RenderStatus::Done => return Ok(job.frames_rendered()),
            RenderStatus::Failed(message) => anyhow::bail!("render failed: {message}"),
            RenderStatus::Cancelled => anyhow::bail!("render cancelled"),
        }
    }
}
