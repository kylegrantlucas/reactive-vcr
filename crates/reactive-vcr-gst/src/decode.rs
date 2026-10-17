//! Whole-file audio decoding for analysis.

use crate::media::{add_source, error_text, make};
use anyhow::{Context as _, Result, anyhow, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use reactive_vcr::audio::features::SAMPLE_RATE;
use std::path::Path;

const PULL_TIMEOUT: gst::ClockTime = gst::ClockTime::from_mseconds(100);

/// Decode the first audio stream of `path` to mono f32 samples at `SAMPLE_RATE`.
pub fn decode_mono_f32(path: &Path) -> Result<Vec<f32>> {
    crate::init()?;
    if !path.exists() {
        bail!("audio file not found: {}", path.display());
    }
    let pipeline = gst::Pipeline::new();
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", 1)
        .field("rate", SAMPLE_RATE as i32)
        .build();
    let sink = AppSink::builder().caps(&caps).sync(false).build();
    let chain = [
        make("queue")?,
        make("audioconvert")?,
        make("audioresample")?,
    ];
    pipeline.add_many(&chain)?;
    pipeline.add(&sink)?;
    gst::Element::link_many(&chain)?;
    chain[2].link(&sink)?;
    let input = chain[0].static_pad("sink").expect("queue has a sink pad");
    add_source(&pipeline, path, None, Some(input), false)?;

    let result = pull_all(&pipeline, &sink)
        .with_context(|| format!("decoding audio from {}", path.display()));
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn pull_all(pipeline: &gst::Pipeline, sink: &AppSink) -> Result<Vec<f32>> {
    let bus = pipeline.bus().expect("pipelines have a bus");
    pipeline.set_state(gst::State::Playing)?;
    let mut samples = Vec::new();
    loop {
        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error])
            && let gst::MessageView::Error(error) = message.view()
        {
            return Err(anyhow!("{}", error_text(error)));
        }
        match sink.try_pull_sample(PULL_TIMEOUT) {
            Some(sample) => {
                let buffer = sample.buffer().context("decoded sample without a buffer")?;
                let map = buffer.map_readable()?;
                let (floats, _) = map.as_slice().as_chunks::<4>();
                samples.extend(floats.iter().map(|bytes| f32::from_le_bytes(*bytes)));
            }
            None if sink.is_eos() => return Ok(samples),
            None => {}
        }
    }
}
