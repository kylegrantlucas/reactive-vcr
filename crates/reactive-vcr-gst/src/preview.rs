//! Preview playback for the desktop app: decoded, unfiltered RGB frames go to a callback in
//! sync with the audio, so the app can apply (and re-apply) the effect itself.

use crate::media::{
    AudioMode, add_source, audio_inputs, error_text, make, seek_range, video_chain, wait_for,
};
use anyhow::{Context as _, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use gstreamer_video::VideoInfo;
use reactive_vcr::timeline::Selection;
use std::path::PathBuf;

const PREROLL_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(30);

/// One decoded RGB frame.
#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub width: usize,
    pub height: usize,
    /// Bytes per row of `data`.
    pub stride: usize,
    pub data: Vec<u8>,
    /// Source position of the frame.
    pub stream_time: gst::ClockTime,
}

#[derive(Debug, Clone)]
pub struct PreviewOptions {
    pub video: PathBuf,
    pub audio: PathBuf,
    pub audio_mode: AudioMode,
    pub has_original_audio: bool,
    pub selection: Selection,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewEvent {
    Ended,
    Error(String),
}

pub struct PreviewPipeline {
    pipeline: gst::Pipeline,
    video_sink: AppSink,
    volume: Option<gst::Element>,
    start: gst::ClockTime,
    stop: gst::ClockTime,
}

impl PreviewPipeline {
    /// Builds the preview and leaves it paused on the first selected frame.
    /// `on_frame` runs on a GStreamer streaming thread.
    pub fn new(
        options: &PreviewOptions,
        on_frame: impl Fn(PreviewFrame) + Send + Sync + 'static,
    ) -> Result<Self> {
        crate::init()?;
        let pipeline = gst::Pipeline::new();
        let (video_in, sized) = video_chain(&pipeline, &options.selection, false)?;
        let video_sink = AppSink::builder()
            .sync(true)
            .max_buffers(1)
            .drop(true)
            .build();
        pipeline.add(&video_sink)?;
        sized.link(&video_sink)?;
        let on_frame = std::sync::Arc::new(on_frame);
        let on_preroll = on_frame.clone();
        video_sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    deliver(&sample, &*on_frame)
                })
                .new_preroll(move |sink| {
                    let sample = sink.pull_preroll().map_err(|_| gst::FlowError::Eos)?;
                    deliver(&sample, &*on_preroll)
                })
                .build(),
        );

        let mode = options.audio_mode.resolve(true, options.has_original_audio);
        let (volume, (driving, original)) = if mode == AudioMode::None {
            (None, (None, None))
        } else {
            let output = [
                make("volume")?,
                make("audioconvert")?,
                make("autoaudiosink")?,
            ];
            pipeline.add_many(&output)?;
            gst::Element::link_many(&output)?;
            let pad = output[0].static_pad("sink").expect("volume sink");
            let inputs = audio_inputs(&pipeline, mode, &pad, false)?;
            (Some(output[0].clone()), inputs)
        };
        add_source(&pipeline, &options.video, Some(video_in), original, true)?;
        if driving.is_some() {
            add_source(&pipeline, &options.audio, None, driving, true)?;
        }

        let (start, stop) = seek_range(&options.selection);
        let preview = Self {
            pipeline,
            video_sink,
            volume,
            start,
            stop,
        };
        preview.pause()?;
        preview.wait_preroll()?;
        preview.seek(start)?;
        preview.wait_preroll()?;
        Ok(preview)
    }

    fn wait_preroll(&self) -> Result<()> {
        let bus = self.pipeline.bus().expect("pipelines have a bus");
        wait_for(
            &bus,
            gst::MessageType::AsyncDone,
            PREROLL_TIMEOUT,
            "preroll the media",
        )
    }

    pub fn play(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .context("starting playback")?;
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Paused)
            .context("pausing playback")?;
        Ok(())
    }

    /// Accurate, flushing seek, clamped to the selection.
    pub fn seek(&self, position: gst::ClockTime) -> Result<()> {
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

    /// Position of the shown frame.
    pub fn position(&self) -> Option<gst::ClockTime> {
        self.video_sink.query_position::<gst::ClockTime>()
    }

    pub fn start(&self) -> gst::ClockTime {
        self.start
    }

    pub fn stop(&self) -> gst::ClockTime {
        self.stop
    }

    /// Audio volume, where 1.0 is unity.
    pub fn set_volume(&self, volume: f64) {
        if let Some(element) = &self.volume {
            element.set_property("volume", volume.clamp(0.0, 4.0));
        }
    }

    /// The next end-of-stream or error, without blocking.
    pub fn poll(&self) -> Option<PreviewEvent> {
        let bus = self.pipeline.bus()?;
        let message = bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])?;
        Some(match message.view() {
            gst::MessageView::Error(error) => PreviewEvent::Error(error_text(error)),
            _ => PreviewEvent::Ended,
        })
    }
}

impl Drop for PreviewPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn deliver(
    sample: &gst::Sample,
    on_frame: &(impl Fn(PreviewFrame) + ?Sized),
) -> Result<gst::FlowSuccess, gst::FlowError> {
    let caps = sample.caps().ok_or(gst::FlowError::NotNegotiated)?;
    let info = VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::NotNegotiated)?;
    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
    let pts = buffer.pts().ok_or(gst::FlowError::Error)?;
    let stream_time = sample
        .segment()
        .and_then(|segment| segment.downcast_ref::<gst::ClockTime>())
        .and_then(|segment| segment.to_stream_time(pts))
        .unwrap_or(pts);
    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
    on_frame(PreviewFrame {
        width: info.width() as usize,
        height: info.height() as usize,
        stride: info.stride()[0] as usize,
        data: map.as_slice().to_vec(),
        stream_time,
    });
    Ok(gst::FlowSuccess::Ok)
}
