//! File rendering: decode, resample to a constant frame rate, apply the effect, and encode.

use crate::filter::{Program, ReactiveFilter};
use crate::media::{
    AudioMode, add_source, audio_inputs, error_text, make, seek_range, video_chain,
};
use anyhow::{Context as _, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use parking_lot::Mutex;
use reactive_vcr::timeline::{Selection, validate_output_is_distinct};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const BUS_POLL: gst::ClockTime = gst::ClockTime::from_mseconds(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputCodec {
    /// H.264 and AAC in MP4; lower quantizers are higher quality (0 is lossless).
    H264 { quantizer: u32 },
    /// Lossless FFV1 and FLAC in Matroska.
    Ffv1,
}

impl OutputCodec {
    pub const DEFAULT_QUANTIZER: u32 = 18;

    /// The codec implied by an output file's extension.
    pub fn from_extension(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "mp4" | "m4v" | "mov" => Some(Self::H264 {
                quantizer: Self::DEFAULT_QUANTIZER,
            }),
            "mkv" => Some(Self::Ffv1),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::H264 { .. } => "mp4",
            Self::Ffv1 => "mkv",
        }
    }
}

#[derive(Clone)]
pub struct RenderSettings {
    pub video: PathBuf,
    pub audio: PathBuf,
    pub output: PathBuf,
    pub codec: OutputCodec,
    pub audio_mode: AudioMode,
    pub selection: Selection,
    /// Whether the video file has its own audio track.
    pub has_original_audio: bool,
    pub program: Arc<Program>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderStatus {
    Running { progress: f64 },
    Done,
    Failed(String),
    Cancelled,
}

/// A render running on GStreamer's threads; poll `status`.
pub struct RenderJob {
    pipeline: gst::Pipeline,
    filter: ReactiveFilter,
    frames: usize,
    status: Arc<Mutex<RenderStatus>>,
    output: PathBuf,
    watcher: Option<JoinHandle<()>>,
}

impl RenderJob {
    pub fn start(settings: RenderSettings) -> Result<Self> {
        crate::init()?;
        validate_output_is_distinct(
            &settings.output,
            &[("video", &settings.video), ("audio", &settings.audio)],
        )?;
        if settings.selection.frames == 0 {
            bail!("the selected range has no frames");
        }
        let built = build(&settings)?;
        let job = Self {
            pipeline: built.pipeline.clone(),
            filter: built.filter.clone(),
            frames: settings.selection.frames,
            status: Arc::new(Mutex::new(RenderStatus::Running { progress: 0.0 })),
            output: settings.output.clone(),
            watcher: None,
        };
        if let Err(error) = job.begin(&settings.selection, built) {
            job.discard();
            return Err(error);
        }
        Ok(job.watch())
    }

    /// Wait until every source has exposed its streams, seek to the selection while the
    /// gates hold back everything before it, then let data through.
    fn begin(&self, selection: &Selection, built: Built) -> Result<()> {
        let (ready_tx, ready_rx) = mpsc::channel();
        for decoder in &built.decoders {
            let ready_tx = ready_tx.clone();
            decoder.connect_no_more_pads(move |_| {
                let _ = ready_tx.send(());
            });
        }
        drop(ready_tx);
        let bus = self.pipeline.bus().expect("pipelines have a bus");
        self.pipeline
            .set_state(gst::State::Paused)
            .context("starting the render pipeline")?;
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        let mut ready = 0;
        while ready < built.decoders.len() {
            if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error])
                && let gst::MessageView::Error(error) = message.view()
            {
                bail!("GStreamer: {}", error_text(error));
            }
            match ready_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(()) => ready += 1,
                Err(mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < deadline => {}
                Err(_) => {
                    bail!("GStreamer did not open the input streams within {READY_TIMEOUT:?}")
                }
            }
        }
        // Muxers don't pass seeks upstream, so seek each branch from its gate. The shared
        // seqnum lets a demuxer feeding both branches seek once.
        let (start, stop) = seek_range(selection);
        let seek = gst::event::Seek::new(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            start,
            gst::SeekType::Set,
            stop,
        );
        for (pad, _) in &built.gates {
            if !pad.push_event(seek.clone()) {
                bail!("seeking to the selected range failed");
            }
        }
        for (pad, probe) in built.gates {
            pad.remove_probe(probe);
        }
        self.pipeline
            .set_state(gst::State::Playing)
            .context("running the render pipeline")?;
        Ok(())
    }

    fn watch(mut self) -> Self {
        let bus = self.pipeline.bus().expect("pipelines have a bus");
        let status = self.status.clone();
        let pipeline = self.pipeline.downgrade();
        self.watcher = Some(std::thread::spawn(move || {
            loop {
                if !matches!(*status.lock(), RenderStatus::Running { .. }) {
                    return;
                }
                let Some(message) = bus.timed_pop_filtered(
                    BUS_POLL,
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                ) else {
                    continue;
                };
                let finished = match message.view() {
                    gst::MessageView::Error(error) => RenderStatus::Failed(error_text(error)),
                    _ => RenderStatus::Done,
                };
                let mut current = status.lock();
                if matches!(*current, RenderStatus::Running { .. }) {
                    *current = finished;
                }
                if let Some(pipeline) = pipeline.upgrade() {
                    let _ = pipeline.set_state(gst::State::Null);
                }
                return;
            }
        }));
        self
    }

    pub fn status(&self) -> RenderStatus {
        match &*self.status.lock() {
            RenderStatus::Running { .. } => RenderStatus::Running {
                progress: (self.filter.frames_processed() as f64 / self.frames as f64).min(1.0),
            },
            other => other.clone(),
        }
    }

    /// Frames the effect has processed so far.
    pub fn frames_rendered(&self) -> u64 {
        self.filter.frames_processed()
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Stop a running render and delete its partial output.
    pub fn cancel(&self) {
        let mut status = self.status.lock();
        if matches!(*status, RenderStatus::Running { .. }) {
            *status = RenderStatus::Cancelled;
            drop(status);
            self.discard();
        }
    }

    fn discard(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        let _ = std::fs::remove_file(&self.output);
    }
}

/// A render pipeline whose decoded data is held at `gates` until the initial seek.
struct Built {
    pipeline: gst::Pipeline,
    filter: ReactiveFilter,
    decoders: Vec<gst::Element>,
    gates: Vec<(gst::Pad, gst::PadProbeId)>,
}

/// Block buffers at `pad` until the probe is removed; flushes still pass and discard them.
fn gate(pad: gst::Pad) -> Result<(gst::Pad, gst::PadProbeId)> {
    let probe = pad
        .add_probe(
            gst::PadProbeType::BLOCK | gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
            |_, _| gst::PadProbeReturn::Ok,
        )
        .context("gating the render pipeline")?;
    Ok((pad, probe))
}

fn property_element(factory: &str, properties: &[(&str, &str)]) -> Result<gst::Element> {
    let element = make(factory)?;
    for (name, value) in properties {
        element.set_property_from_str(name, value);
    }
    Ok(element)
}

fn build(settings: &RenderSettings) -> Result<Built> {
    let pipeline = gst::Pipeline::new();
    let filter = ReactiveFilter::new(settings.program.clone());
    filter.limit_frames(settings.selection.frames);
    let (muxer, video_encoder, audio_encoder) = match settings.codec {
        OutputCodec::H264 { quantizer } => (
            property_element("mp4mux", &[("faststart", "true")])?,
            vec![
                property_element("capsfilter", &[("caps", "video/x-raw,format=I420")])?,
                property_element(
                    "x264enc",
                    &[
                        ("pass", "quant"),
                        ("quantizer", &quantizer.min(51).to_string()),
                        ("speed-preset", "medium"),
                    ],
                )?,
            ],
            make("avenc_aac")?,
        ),
        OutputCodec::Ffv1 => (
            make("matroskamux")?,
            vec![
                // BGRx keeps FFV1 lossless for RGB frames.
                property_element("capsfilter", &[("caps", "video/x-raw,format=BGRx")])?,
                make("avenc_ffv1")?,
            ],
            make("flacenc")?,
        ),
    };
    let file = gst::ElementFactory::make("filesink")
        .property("location", settings.output.to_string_lossy().as_ref())
        .property("async", false)
        .build()
        .context("GStreamer element `filesink` is unavailable")?;
    pipeline.add_many([&muxer, &file])?;
    muxer.link(&file)?;

    let (video_in, sized) = video_chain(&pipeline, &settings.selection, true)?;
    let mut video = vec![filter.clone().upcast(), make("videoconvert")?];
    video.extend(video_encoder);
    pipeline.add_many(&video)?;
    sized.link(&video[0])?;
    gst::Element::link_many(&video)?;
    video.last().expect("video chain").link(&muxer)?;
    let mut gates = vec![gate(video_in.clone())?];

    let mode = settings
        .audio_mode
        .resolve(true, settings.has_original_audio);
    let (driving, original) = if mode == AudioMode::None {
        (None, None)
    } else {
        let audio = [make("audioconvert")?, audio_encoder];
        pipeline.add_many(&audio)?;
        gst::Element::link_many(&audio)?;
        audio[1].link(&muxer)?;
        let input = audio[0].static_pad("sink").expect("audioconvert sink");
        let inputs = audio_inputs(&pipeline, mode, &input, true)?;
        gates.push(gate(input)?);
        inputs
    };
    let mut decoders = vec![add_source(
        &pipeline,
        &settings.video,
        Some(video_in),
        original,
        false,
    )?];
    if driving.is_some() {
        decoders.push(add_source(
            &pipeline,
            &settings.audio,
            None,
            driving,
            false,
        )?);
    }
    Ok(Built {
        pipeline,
        filter,
        decoders,
        gates,
    })
}

impl Drop for RenderJob {
    fn drop(&mut self) {
        self.cancel();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}
