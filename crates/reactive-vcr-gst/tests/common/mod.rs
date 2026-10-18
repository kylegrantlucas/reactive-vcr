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
    clip_with(dir, seconds, with_audio, "width=160,height=120")
}

/// Like `clip`, with extra video caps fields such as size and pixel aspect ratio.
pub fn clip_with(dir: &Path, seconds: u32, with_audio: bool, caps: &str) -> PathBuf {
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
        "videotestsrc pattern=smpte num-buffers={frames} ! video/x-raw,{caps},framerate=30/1 \
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
    decoded(path, "videoconvert ! video/x-raw,format=RGB").1
}

/// The decoded video of `path` after the gst-launch fragment `convert`: the caps of its
/// first frame, and every frame's planes concatenated without row padding.
pub fn decoded(path: &Path, convert: &str) -> (gstreamer_video::VideoInfo, Vec<Vec<u8>>) {
    reactive_vcr_gst::init().unwrap();
    let pipeline = gst::parse::launch(&format!(
        "filesrc location={} ! decodebin ! {convert} ! appsink name=sink sync=false caps=video/x-raw",
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
    single_threaded_libav(pipeline.upcast_ref());
    pipeline.set_state(gst::State::Playing).unwrap();
    let mut first = None;
    let mut frames = Vec::new();
    while let Ok(sample) = sink.pull_sample() {
        let info = gstreamer_video::VideoInfo::from_caps(sample.caps().unwrap()).unwrap();
        let frame = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(
            sample.buffer().unwrap(),
            &info,
        )
        .unwrap();
        let mut bytes = Vec::new();
        for plane in 0..info.n_planes() {
            let component = info
                .format_info()
                .plane()
                .iter()
                .position(|&owner| owner == plane)
                .unwrap() as u8;
            let row = info.comp_width(component) as usize * info.comp_pstride(component) as usize;
            let stride = info.stride()[plane as usize] as usize;
            for line in frame
                .plane_data(plane)
                .unwrap()
                .chunks(stride)
                .take(info.comp_height(component) as usize)
            {
                bytes.extend_from_slice(&line[..row]);
            }
        }
        frames.push(bytes);
        first.get_or_insert(info);
    }
    pipeline.set_state(gst::State::Null).unwrap();
    (first.expect("a decoded video frame"), frames)
}

/// gst-libav 1.24's frame-threaded FFV1 decoder sometimes loses the last frame at end of
/// stream, which would make frame counts flaky; decode it on one thread instead.
pub fn single_threaded_libav(bin: &gst::Bin) {
    bin.connect_deep_element_added(|_, _, element| {
        if element
            .factory()
            .is_some_and(|factory| factory.name() == "avdec_ffv1")
        {
            element.set_property("max-threads", 1i32);
        }
    });
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
