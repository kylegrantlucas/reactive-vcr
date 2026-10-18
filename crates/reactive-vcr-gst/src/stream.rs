//! Real-time streaming: live video through the effect, driven by live audio analysis.
//! Edits to the mapping file reload live.

use crate::filter::{KEY_MESSAGE, ReactiveFilter, VIDEO_EOS_MESSAGE};
use crate::media::{error_text, is_window_closed, make, sink, source};
use crate::play::{BUS_POLL, Control, control_for};
use anyhow::{Context as _, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use gstreamer_video as gst_video;
use parking_lot::Mutex;
use reactive_vcr::audio::features::SAMPLE_RATE;
use reactive_vcr::live::LiveProgram;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::watch::MappingWatcher;
use std::path::PathBuf;
use std::sync::Arc;

/// Frame rate assumed until the video caps report one.
const DEFAULT_FPS: f64 = 30.0;

#[derive(Debug, Clone)]
pub struct StreamOptions {
    pub map: PathBuf,
    /// GStreamer video source description, such as `autovideosrc`.
    pub video_source: String,
    /// GStreamer audio source description that drives the effect, such as `autoaudiosrc`.
    pub audio_source: String,
    /// Processing height; the width follows the source's aspect ratio.
    pub lines: u32,
    /// GStreamer video sink description, such as `autovideosink`.
    pub video_sink: String,
    /// GStreamer sink for the driving audio, such as `fakesink` or `autoaudiosink`.
    pub audio_sink: String,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamSummary {
    /// Frames the effect processed.
    pub frames: u64,
}

/// Stream live video with the effect driven by live audio, until the video ends, the window
/// closes, or the user quits.
pub fn stream(o: &StreamOptions) -> Result<StreamSummary> {
    crate::init()?;
    let catalog = Arc::new(ParamCatalog::new());
    let mut watcher = MappingWatcher::new(o.map.clone());
    let map = watcher.load(&catalog)?;
    let live = Arc::new(Mutex::new(LiveProgram::new(
        catalog.clone(),
        Arc::new(map),
        SAMPLE_RATE,
        DEFAULT_FPS,
    )));

    let pipeline = gst::Pipeline::new();
    let filter = ReactiveFilter::new_live(live.clone());
    let video_sink = sink(&o.video_sink)?;
    build_video(&pipeline, o, &filter, &video_sink)?;
    build_audio(&pipeline, o, &live)?;

    if !o.quiet {
        eprintln!(
            "streaming {} at {} lines · q quit · edits to {} reload live",
            o.video_source,
            o.lines,
            o.map.display()
        );
    }
    let result = event_loop(
        &pipeline,
        &video_sink,
        &mut watcher,
        &catalog,
        &live,
        o.quiet,
    );
    let _ = pipeline.set_state(gst::State::Null);
    result.map(|()| StreamSummary {
        frames: filter.frames_processed(),
    })
}

/// `source ! queue(leaky, 1 buffer) ! videoconvert ! videoscale ! RGB at `lines` ! filter !
/// videoconvert ! sink`. The leaky queue drops stale frames when processing falls behind
/// capture, which bounds latency.
fn build_video(
    pipeline: &gst::Pipeline,
    o: &StreamOptions,
    filter: &ReactiveFilter,
    video_sink: &gst::Element,
) -> Result<()> {
    let queue = make("queue")?;
    queue.set_property_from_str("leaky", "downstream");
    queue.set_property("max-size-buffers", 1u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", 0u64);
    let mut elements = vec![source(&o.video_source)?, queue];
    for factory in ["videoconvert", "videoscale"] {
        let element = make(factory)?;
        element.set_property("n-threads", 0u32);
        elements.push(element);
    }
    let caps = gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgb)
        .height(o.lines as i32)
        .build();
    elements.push(
        gst::ElementFactory::make("capsfilter")
            .property("caps", caps)
            .build()
            .context("GStreamer element `capsfilter` is unavailable")?,
    );
    elements.push(filter.clone().upcast());
    elements.push(make("videoconvert")?);
    elements.push(video_sink.clone());
    pipeline.add_many(&elements)?;
    gst::Element::link_many(&elements)?;
    Ok(())
}

/// `source ! tee`, with one branch analyzed as mono f32 at `SAMPLE_RATE` and one sent to the
/// audio sink.
fn build_audio(
    pipeline: &gst::Pipeline,
    o: &StreamOptions,
    live: &Arc<Mutex<LiveProgram>>,
) -> Result<()> {
    let input = [source(&o.audio_source)?, make("tee")?];
    pipeline.add_many(&input)?;
    gst::Element::link_many(&input)?;

    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", 1)
        .field("rate", SAMPLE_RATE as i32)
        .build();
    let analysis = AppSink::builder().caps(&caps).sync(false).build();
    let live = live.clone();
    let mut scratch = Vec::new();
    analysis.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let (floats, _) = map.as_slice().as_chunks::<4>();
                scratch.clear();
                scratch.extend(floats.iter().map(|bytes| f32::from_le_bytes(*bytes)));
                live.lock().push_audio(&scratch);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    let analysis_branch = [
        make("queue")?,
        make("audioconvert")?,
        make("audioresample")?,
        analysis.upcast(),
    ];
    let monitor_branch = [
        make("queue")?,
        make("audioconvert")?,
        make("audioresample")?,
        sink(&o.audio_sink)?,
    ];
    for branch in [&analysis_branch, &monitor_branch] {
        pipeline.add_many(branch)?;
        gst::Element::link_many(branch)?;
        input[1].link(&branch[0])?;
    }
    Ok(())
}

fn event_loop(
    pipeline: &gst::Pipeline,
    video_sink: &gst::Element,
    watcher: &mut MappingWatcher,
    catalog: &ParamCatalog,
    live: &Mutex<LiveProgram>,
    quiet: bool,
) -> Result<()> {
    // Live sources don't preroll, so there is nothing to wait for before playing.
    pipeline
        .set_state(gst::State::Playing)
        .context("starting the stream")?;
    let bus = pipeline.bus().expect("pipelines have a bus");
    loop {
        if let Some(message) = bus.timed_pop(BUS_POLL) {
            match message.view() {
                gst::MessageView::Error(error) if is_window_closed(error, video_sink) => {
                    return Ok(());
                }
                gst::MessageView::Error(error) => {
                    return Err(anyhow!("GStreamer: {}", error_text(error)));
                }
                gst::MessageView::Eos(_) => return Ok(()),
                gst::MessageView::Application(application) => {
                    let Some(structure) = application.structure() else {
                        continue;
                    };
                    if structure.name() == VIDEO_EOS_MESSAGE {
                        return Ok(());
                    }
                    if structure.name() == KEY_MESSAGE {
                        let key = structure.get::<&str>("key").unwrap_or_default();
                        if control_for(key) == Some(Control::Quit) {
                            return Ok(());
                        }
                    }
                }
                _ => {}
            }
        }
        match watcher.poll(catalog) {
            Some(Ok(map)) => {
                live.lock().set_mapping(Arc::new(map));
                if !quiet {
                    eprintln!("reloaded {}", watcher.path().display());
                }
            }
            Some(Err(error)) => {
                eprintln!("{error:#}\nkeeping the previous mapping until the next edit");
            }
            None => {}
        }
    }
}
