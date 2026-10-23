//! Real-time streaming: live video through the effect, driven by live audio analysis.
//! Edits to the mapping file reload live.

use crate::filter::{KEY_MESSAGE, ReactiveFilter, VIDEO_EOS_MESSAGE};
use crate::media::{error_text, is_window_closed, make, sink, source};
use crate::play::{BUS_POLL, Control, control_for};
use crate::telemetry::{Telemetry, TelemetryOptions};
use anyhow::{Context as _, Result, anyhow};
use gstreamer as gst;
use gstreamer::glib;
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
use std::time::Instant;

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
    /// Gate the driving audio's analysis at this level in dBFS; see `LiveProgram::set_gate`.
    pub gate_db: Option<f32>,
    /// Where to send UDP telemetry, if anywhere.
    pub telemetry: Option<TelemetryOptions>,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamSummary {
    /// Frames the effect processed.
    pub frames: u64,
}

/// Stream live video with the effect driven by live audio, until the video ends, the window
/// closes, or the user quits. With telemetry, a fatal error is also sent before returning.
pub fn stream(o: &StreamOptions) -> Result<StreamSummary> {
    let telemetry = o
        .telemetry
        .as_ref()
        .map(Telemetry::connect)
        .transpose()?
        .map(Arc::new);
    let result = run(o, telemetry.as_ref());
    if let (Err(error), Some(telemetry)) = (&result, &telemetry) {
        telemetry.error(&format!("{error:#}"));
    }
    result
}

fn run(o: &StreamOptions, telemetry: Option<&Arc<Telemetry>>) -> Result<StreamSummary> {
    crate::init()?;
    let catalog = Arc::new(ParamCatalog::new());
    let mut watcher = MappingWatcher::new(o.map.clone());
    let map = watcher.load(&catalog)?;
    let mut program = LiveProgram::new(catalog.clone(), Arc::new(map), SAMPLE_RATE, DEFAULT_FPS);
    program.set_gate(o.gate_db);
    let live = Arc::new(Mutex::new(program));

    let pipeline = gst::Pipeline::new();
    let filter = ReactiveFilter::new_live(live.clone());
    let video_sink = sink(&o.video_sink)?;
    let queue = build_video(&pipeline, o, &filter, &video_sink)?;
    build_audio(&pipeline, o, &live)?;
    if let Some(telemetry) = telemetry {
        observe(&filter, &queue, &live, telemetry);
    }

    if !o.quiet {
        eprintln!(
            "streaming {} at {} lines · q quit · edits to {} reload live",
            o.video_source,
            o.lines,
            o.map.display()
        );
    }
    // Live sources don't preroll, so there is nothing to wait for before playing.
    let result = pipeline
        .set_state(gst::State::Playing)
        .context("starting the stream")
        .and_then(|_| {
            if let Some(telemetry) = telemetry {
                telemetry.start(&o.map, o.lines);
            }
            event_loop(
                &pipeline,
                &video_sink,
                &mut watcher,
                &catalog,
                &live,
                telemetry.map(Arc::as_ref),
                o.quiet,
            )
        });
    let _ = pipeline.set_state(gst::State::Null);
    result.map(|()| StreamSummary {
        frames: filter.frames_processed(),
    })
}

/// Feed `telemetry` from the frame path: each frame's effect time and the frame count from
/// `filter`, and every frame the latency queue discards.
///
/// The effect time runs from a frame entering `filter` to its result leaving, both on the
/// streaming thread, so it covers live analysis, modulation, and the effect itself. A leaky
/// queue holding at most one frame emits `overrun` once for each frame it discards; a
/// non-leaky one emits it while it waits, which drops nothing and is not counted.
fn observe(
    filter: &ReactiveFilter,
    queue: &gst::Element,
    live: &Arc<Mutex<LiveProgram>>,
    telemetry: &Arc<Telemetry>,
) {
    let entered = Arc::new(Mutex::new(None::<Instant>));
    let input = filter.static_pad("sink").expect("filters have an input");
    let output = filter.static_pad("src").expect("filters have an output");
    {
        let entered = entered.clone();
        input.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            *entered.lock() = Some(Instant::now());
            gst::PadProbeReturn::Ok
        });
    }
    {
        let filter = filter.downgrade();
        let live = live.clone();
        let telemetry = telemetry.clone();
        output.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            if let (Some(filter), Some(entered)) = (filter.upgrade(), entered.lock().take()) {
                telemetry.frame(entered.elapsed(), filter.frames_processed(), &live);
            }
            gst::PadProbeReturn::Ok
        });
    }
    let telemetry = telemetry.clone();
    queue.connect("overrun", false, move |args| {
        let queue = args[0].get::<gst::Element>().ok()?;
        let leaky = queue.property_value("leaky");
        if glib::EnumValue::from_value(&leaky).is_some_and(|(_, value)| value.nick() != "no") {
            telemetry.frame_dropped();
        }
        None
    });
}

/// `source ! queue(1 buffer) ! videoconvert ! videoscale ! RGB at `lines` ! filter !
/// videoconvert ! sink`; returns the queue. How the queue treats the source follows from
/// whether it is live, which `pace_source` decides at its first frame.
fn build_video(
    pipeline: &gst::Pipeline,
    o: &StreamOptions,
    filter: &ReactiveFilter,
    video_sink: &gst::Element,
) -> Result<gst::Element> {
    let queue = make("queue")?;
    queue.set_property("max-size-buffers", 1u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", 0u64);
    let source = source(&o.video_source)?;
    pace_source(&source, &queue);
    let mut elements = vec![source, queue.clone()];
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
    Ok(queue)
}

/// At the source's first frame, ask upstream whether it is live.
///
/// A live source, such as a camera, keeps capturing whether or not anyone keeps up, so the
/// queue becomes leaky and drops stale frames when processing falls behind, which bounds
/// latency. A non-live source, such as a file or a pipe, produces as fast as it is consumed,
/// so the queue holds it back and the sink paces every frame by its timestamp. Its
/// timestamps start at zero whenever its first frame arrives, which may be well after the
/// stream started; when that frame is late, the source's output is offset by the lateness
/// so it plays from that moment in real time instead of being dropped as late.
fn pace_source(source: &gst::Element, queue: &gst::Element) {
    let output = source.static_pad("src").expect("sources have an output");
    let source = source.downgrade();
    let queue = queue.downgrade();
    output.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let mut latency = gst::query::Latency::new();
        // A source that can't say keeps the latency-bounding behavior.
        let live = !pad.query(&mut latency) || latency.result().0;
        if live {
            if let Some(queue) = queue.upgrade() {
                queue.set_property_from_str("leaky", "downstream");
            }
        } else if let Some(buffer) = info.buffer()
            && let Some(source) = source.upgrade()
            && let Some(late) = lateness(pad, &source, buffer)
        {
            pad.set_offset(late.nseconds() as i64);
        }
        gst::PadProbeReturn::Remove
    });
}

/// How far `buffer`, about to leave `pad` of `source`, is behind the running time, if at all.
fn lateness(
    pad: &gst::Pad,
    source: &gst::Element,
    buffer: &gst::BufferRef,
) -> Option<gst::ClockTime> {
    // Before it plays, the source has no running time yet and its base time may be stale.
    if source.current_state() != gst::State::Playing {
        return None;
    }
    let now = source.clock()?.time().checked_sub(source.base_time()?)?;
    let segment = pad.sticky_event::<gst::event::Segment>(0)?;
    let due = segment
        .segment()
        .downcast_ref::<gst::ClockTime>()?
        .to_running_time(buffer.pts()?)?;
    now.checked_sub(due).filter(|late| !late.is_zero())
}

/// Analyze mono f32 at `SAMPLE_RATE`, adding a tee and monitor branch only for an
/// explicitly configured sink other than the default `fakesink`.
fn build_audio(
    pipeline: &gst::Pipeline,
    o: &StreamOptions,
    live: &Arc<Mutex<LiveProgram>>,
) -> Result<()> {
    let input = source(&o.audio_source)?;
    pipeline.add(&input)?;

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
        make("audioconvert")?,
        make("audioresample")?,
        analysis.upcast(),
    ];
    pipeline.add_many(&analysis_branch)?;
    gst::Element::link_many(&analysis_branch)?;
    if o.audio_sink == "fakesink" {
        // The default fakesink and analysis sink are both unsynced. Keep source-driven
        // pacing and the analysis caps without a second branch or a queue.
        input.link(&analysis_branch[0])?;
    } else {
        let tee = make("tee")?;
        let analysis_queue = make("queue")?;
        let monitor_branch = [
            make("queue")?,
            make("audioconvert")?,
            make("audioresample")?,
            sink(&o.audio_sink)?,
        ];
        pipeline.add_many([&tee, &analysis_queue])?;
        pipeline.add_many(&monitor_branch)?;
        gst::Element::link_many([&input, &tee, &analysis_queue, &analysis_branch[0]])?;
        gst::Element::link_many(&monitor_branch)?;
        tee.link(&monitor_branch[0])?;
    }
    Ok(())
}

fn event_loop(
    pipeline: &gst::Pipeline,
    video_sink: &gst::Element,
    watcher: &mut MappingWatcher,
    catalog: &ParamCatalog,
    live: &Mutex<LiveProgram>,
    telemetry: Option<&Telemetry>,
    quiet: bool,
) -> Result<()> {
    let bus = pipeline.bus().expect("pipelines have a bus");
    let mut draining = false;
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
                    if structure.name() == VIDEO_EOS_MESSAGE && !draining {
                        // This message comes from the effect, before downstream encoders,
                        // queues and muxers have drained. End live audio too, then wait for
                        // the pipeline's EOS message from all sinks before setting Null.
                        draining = true;
                        pipeline.send_event(gst::event::Eos::new());
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
                if let Some(telemetry) = telemetry {
                    telemetry.set_mapping_error(None);
                }
                if !quiet {
                    eprintln!("reloaded {}", watcher.path().display());
                }
            }
            Some(Err(error)) => {
                if let Some(telemetry) = telemetry {
                    telemetry.set_mapping_error(Some(format!("{error:#}")));
                }
                eprintln!("{error:#}\nkeeping the previous mapping until the next edit");
            }
            None => {}
        }
    }
}
