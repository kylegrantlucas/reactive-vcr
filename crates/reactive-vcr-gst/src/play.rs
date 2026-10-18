//! Real-time playback in a GStreamer video sink window. GStreamer decodes and clocks the media;
//! the filter applies the same per-frame values that `render` writes, looked up by each
//! frame's timestamp. Edits to the mapping file reload live.

use crate::decode::decode_mono_f32;
use crate::filter::{KEY_MESSAGE, Program, ReactiveFilter, VIDEO_EOS_MESSAGE};
use crate::media::{
    AudioMode, add_source, audio_inputs, error_text, is_window_closed, make, seek_range, sink,
    video_chain, wait_for,
};
use crate::probe::probe;
use anyhow::{Context as _, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use reactive_vcr::mapping::CompiledMapping;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::{FeatureCache, Selection, select_timeline};
use reactive_vcr::watch::MappingWatcher;
use std::path::PathBuf;
use std::sync::Arc;

const SEEK_STEP: gst::ClockTime = gst::ClockTime::from_seconds(5);
pub(crate) const BUS_POLL: gst::ClockTime = gst::ClockTime::from_mseconds(100);
const PREROLL_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(30);

#[derive(Debug, Clone)]
pub struct PlayOptions {
    pub video: PathBuf,
    pub audio: PathBuf,
    pub map: PathBuf,
    pub preview: Option<f64>,
    pub start_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub audio_mode: AudioMode,
    /// Restart at the end instead of exiting.
    pub looping: bool,
    /// GStreamer video sink description, such as `autovideosink` or `glimagesink sync=true`.
    pub video_sink: String,
    /// GStreamer audio sink description, such as `autoaudiosink`.
    pub audio_sink: String,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaySummary {
    /// Frames the effect processed, including repeats after seeks and loops.
    pub frames: u64,
}

/// Play the selected video timeline with the effect driven by the audio file, until the
/// video ends (or forever with `looping`), the window closes, or the user quits.
pub fn play(o: &PlayOptions) -> Result<PlaySummary> {
    crate::init()?;
    let info = probe(&o.video)?;
    let selection = select_timeline(&info, o.preview, o.start_s, o.duration_s)?;
    let samples = decode_mono_f32(&o.audio)?;
    let mut programs = ProgramSource::new(o.map.clone(), samples, selection);
    let program = programs.load()?;

    let mode = o.audio_mode.resolve(true, info.has_audio);
    let player = Player::build(o, &selection, mode, program)?;
    if !o.quiet {
        eprintln!(
            "playing {} at {}x{} @ {:.3} fps\n\
             space pause · ←/→ seek 5 s · home restart · q quit · edits to {} reload live",
            o.video.display(),
            selection.width,
            selection.height,
            selection.fps(),
            o.map.display()
        );
    }
    player.run(&mut programs, o.quiet)
}

/// Builds programs from the mapping and rebuilds them when it or its presets change.
struct ProgramSource {
    watcher: MappingWatcher,
    catalog: Arc<ParamCatalog>,
    samples: Vec<f32>,
    selection: Selection,
    features: FeatureCache,
}

impl ProgramSource {
    fn new(map: PathBuf, samples: Vec<f32>, selection: Selection) -> Self {
        Self {
            watcher: MappingWatcher::new(map),
            catalog: Arc::new(ParamCatalog::new()),
            samples,
            selection,
            features: FeatureCache::default(),
        }
    }

    fn load(&mut self) -> Result<Arc<Program>> {
        let map = self.watcher.load(&self.catalog)?;
        self.build(map)
    }

    fn build(&mut self, map: CompiledMapping) -> Result<Arc<Program>> {
        let features = self
            .features
            .features(&self.samples, &map, &self.selection)?;
        Ok(Arc::new(Program::new(
            self.catalog.clone(),
            &map,
            &features,
            &self.selection,
        )))
    }

    /// A rebuilt program once any watched file changes. A failed load is reported once and
    /// retried after the next change.
    fn poll(&mut self) -> Option<Result<Arc<Program>>> {
        self.watcher
            .poll(&self.catalog)
            .map(|result| result.and_then(|map| self.build(map)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Control {
    TogglePause,
    Back,
    Forward,
    Restart,
    Quit,
}

/// Navigation key names, as video sinks report them (X11 keysym style).
pub(crate) fn control_for(key: &str) -> Option<Control> {
    Some(match key {
        "space" | " " => Control::TogglePause,
        "Left" => Control::Back,
        "Right" => Control::Forward,
        "Home" => Control::Restart,
        "q" | "Q" | "Escape" => Control::Quit,
        _ => return None,
    })
}

struct Player {
    pipeline: gst::Pipeline,
    filter: ReactiveFilter,
    video_sink: gst::Element,
    start: gst::ClockTime,
    stop: gst::ClockTime,
    looping: bool,
}

impl Player {
    fn build(
        o: &PlayOptions,
        selection: &Selection,
        mode: AudioMode,
        program: Arc<Program>,
    ) -> Result<Self> {
        let pipeline = gst::Pipeline::new();
        let filter = ReactiveFilter::new(program);
        filter.limit_frames(selection.frames);
        let (video_in, sized) = video_chain(&pipeline, selection, false)?;
        let video_sink = sink(&o.video_sink)?;
        let tail = [
            filter.clone().upcast(),
            make("videoconvert")?,
            video_sink.clone(),
        ];
        pipeline.add_many(&tail)?;
        sized.link(&tail[0])?;
        gst::Element::link_many(&tail)?;

        let (driving, original) = if mode == AudioMode::None {
            (None, None)
        } else {
            let output = [make("audioconvert")?, sink(&o.audio_sink)?];
            pipeline.add_many(&output)?;
            gst::Element::link_many(&output)?;
            let pad = output[0].static_pad("sink").expect("audioconvert sink");
            audio_inputs(&pipeline, mode, &pad, false)?
        };
        add_source(&pipeline, &o.video, Some(video_in), original, true)?;
        if driving.is_some() {
            add_source(&pipeline, &o.audio, None, driving, true)?;
        }

        let (start, stop) = seek_range(selection);
        Ok(Self {
            pipeline,
            filter,
            video_sink,
            start,
            stop,
            looping: o.looping,
        })
    }

    fn bus(&self) -> gst::Bus {
        self.pipeline.bus().expect("pipelines have a bus")
    }

    /// Wait for a state change or flushing seek to finish prerolling.
    fn wait_preroll(&self) -> Result<()> {
        wait_for(
            &self.bus(),
            gst::MessageType::AsyncDone,
            PREROLL_TIMEOUT,
            "preroll the media",
        )
    }

    fn seek(&self, position: gst::ClockTime) -> Result<()> {
        let position = position.clamp(self.start, self.stop);
        self.pipeline
            .seek(
                1.0,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                position,
                gst::SeekType::Set,
                self.stop,
            )
            .with_context(|| format!("seeking to {position}"))
    }

    /// The shown frame's position; other sinks may report positions ahead of it.
    fn position(&self) -> gst::ClockTime {
        self.video_sink
            .query_position::<gst::ClockTime>()
            .unwrap_or(self.start)
    }

    fn set_state(&self, state: gst::State) -> Result<()> {
        self.pipeline
            .set_state(state)
            .map(drop)
            .with_context(|| format!("switching playback to {state:?}"))
    }

    fn run(self, programs: &mut ProgramSource, quiet: bool) -> Result<PlaySummary> {
        let result = self.event_loop(programs, quiet);
        let _ = self.pipeline.set_state(gst::State::Null);
        result.map(|()| PlaySummary {
            frames: self.filter.frames_processed(),
        })
    }

    fn event_loop(&self, programs: &mut ProgramSource, quiet: bool) -> Result<()> {
        self.set_state(gst::State::Paused)?;
        self.wait_preroll()?;
        self.seek(self.start)?;
        self.wait_preroll()?;
        self.set_state(gst::State::Playing)?;
        let bus = self.bus();
        let mut paused = false;
        loop {
            if let Some(message) = bus.timed_pop(BUS_POLL) {
                match message.view() {
                    gst::MessageView::Error(error) if is_window_closed(error, &self.video_sink) => {
                        return Ok(());
                    }
                    gst::MessageView::Error(error) => {
                        return Err(anyhow!("GStreamer: {}", error_text(error)));
                    }
                    gst::MessageView::Application(application) => {
                        let Some(structure) = application.structure() else {
                            continue;
                        };
                        if structure.name() == VIDEO_EOS_MESSAGE {
                            if !self.looping {
                                return Ok(());
                            }
                            self.seek(self.start)?;
                        } else if structure.name() == KEY_MESSAGE {
                            let key = structure.get::<&str>("key").unwrap_or_default();
                            match control_for(key) {
                                Some(Control::Quit) => return Ok(()),
                                Some(Control::TogglePause) => {
                                    paused = !paused;
                                    self.set_state(if paused {
                                        gst::State::Paused
                                    } else {
                                        gst::State::Playing
                                    })?;
                                }
                                Some(Control::Back) => {
                                    self.seek(self.position().saturating_sub(SEEK_STEP))?
                                }
                                Some(Control::Forward) => self.seek(self.position() + SEEK_STEP)?,
                                Some(Control::Restart) => self.seek(self.start)?,
                                None => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            match programs.poll() {
                Some(Ok(program)) => {
                    self.filter.set_program(program);
                    if paused {
                        // Re-preroll the paused frame so the new mapping is visible.
                        self.seek(self.position())?;
                    }
                    if !quiet {
                        eprintln!("reloaded {}", programs.watcher.path().display());
                    }
                }
                Some(Err(error)) => {
                    eprintln!("{error:#}\nkeeping the previous mapping until the next edit");
                }
                None => {}
            }
        }
    }
}
