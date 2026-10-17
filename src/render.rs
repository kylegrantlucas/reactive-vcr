use crate::audio::decode::{SAMPLE_RATE, decode_mono_f32, tool_available};
use crate::audio::features::{FeatureParams, extract, frames_for};
use crate::effect::FrameProcessor;
use crate::mapping;
use crate::modulate::Modulator;
use crate::params::ParamCatalog;
use crate::probe::probe;
use crate::video::decoder::{DecodeOptions, FrameReader, even_scaled};
use crate::video::encoder::{AudioMode, EncodeOptions, FrameWriter};
use anyhow::{Context as _, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub video: PathBuf,
    pub audio: PathBuf,
    pub map: PathBuf,
    pub output: PathBuf,
    pub preview: Option<f64>,
    pub start_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub audio_mode: AudioMode,
    pub extra_out_args: Vec<String>,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderSummary {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

const CHANNEL_DEPTH: usize = 4;

fn validate_timeline(o: &RenderOptions) -> Result<()> {
    if o.preview
        .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1.0)
    {
        bail!("--preview must be finite and within (0, 1]");
    }
    if o.start_s
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        bail!("--start must be finite and nonnegative");
    }
    if o.duration_s
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        bail!("--duration must be finite and positive");
    }
    Ok(())
}

fn canonical_existing(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn paths_alias(left: &Path, right: &Path) -> bool {
    if canonical_existing(left)
        .zip(canonical_existing(right))
        .is_some_and(|(left, right)| left == right)
    {
        return true;
    }
    #[cfg(unix)]
    {
        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }
    false
}

pub(crate) fn validate_output_is_distinct(output: &Path, inputs: &[(&str, &Path)]) -> Result<()> {
    for &(label, input) in inputs {
        if paths_alias(output, input) {
            bail!(
                "output {} aliases {label} input {}; choose a distinct output path",
                output.display(),
                input.display()
            );
        }
    }
    Ok(())
}

fn join_worker(thread: std::thread::JoinHandle<Result<()>>, name: &str) -> Result<()> {
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("{name} thread panicked"))?
        .with_context(|| format!("{name} worker failed"))
}

/// Render the selected video timeline while driving the effect from the selected audio timeline.
pub fn render(o: &RenderOptions) -> Result<RenderSummary> {
    validate_timeline(o)?;
    for tool in ["ffmpeg", "ffprobe"] {
        if !tool_available(tool) {
            bail!("{tool} not found on PATH; install ffmpeg");
        }
    }

    let info = probe(&o.video)?;
    let scale = o.preview.unwrap_or(1.0);
    let width = even_scaled(info.width, scale);
    let height = even_scaled(info.height, scale);
    let start_s = o.start_s.unwrap_or(0.0);
    let video_span = (info.duration_s - start_s).max(0.0);
    let duration_s = o
        .duration_s
        .map_or(video_span, |duration| duration.min(video_span));
    if duration_s <= 0.0 {
        bail!(
            "selected video range is empty: start {start_s}s is at or beyond {}s",
            info.duration_s
        );
    }
    let selected_duration = Some(duration_s);
    let expected_frames = (duration_s * info.fps).round() as usize;

    let catalog = ParamCatalog::new();
    let map = mapping::load(&o.map, &catalog)?;
    validate_output_is_distinct(
        &o.output,
        &[
            ("video", &o.video),
            ("audio", &o.audio),
            ("mapping", &o.map),
        ],
    )?;
    validate_output_is_distinct(&o.output, &map.preset_inputs())?;
    let samples = decode_mono_f32(&o.audio)?;
    if frames_for(
        samples.len(),
        SAMPLE_RATE,
        info.fps,
        start_s,
        selected_duration,
    ) == 0
    {
        bail!("audio file has no content after start offset {start_s}s");
    }
    let features = extract(
        &samples,
        SAMPLE_RATE,
        &FeatureParams {
            fps: info.fps,
            frames: expected_frames.max(1),
            start_s,
            bands: map.bands.iter().map(|(_, range)| *range).collect(),
            normalize: map.normalize,
            onset_sensitivity: map.onset_sensitivity,
        },
    );
    let mut modulator = Modulator::new(&catalog, &map, &features, info.fps);

    let reader = FrameReader::spawn(&DecodeOptions {
        input: o.video.clone(),
        fps: info.fps,
        start_s: o.start_s,
        duration_s: selected_duration,
        width,
        height,
    })?;
    let writer = FrameWriter::spawn(&EncodeOptions {
        output: o.output.clone(),
        width,
        height,
        fps: info.fps,
        start_s: o.start_s,
        driving_audio: Some(o.audio.clone()),
        original_video: Some(o.video.clone()),
        original_has_audio: info.has_audio,
        audio_mode: o.audio_mode,
        extra_out_args: o.extra_out_args.clone(),
    })?;
    let frame_bytes = reader.frame_bytes();
    let (in_tx, in_rx) = crossbeam_channel::bounded::<Vec<u8>>(CHANNEL_DEPTH);
    let (out_tx, out_rx) = crossbeam_channel::bounded::<Vec<u8>>(CHANNEL_DEPTH);

    let read_thread = std::thread::spawn(move || -> Result<()> {
        let mut reader = reader;
        loop {
            let mut frame = vec![0; frame_bytes];
            if !reader.read_frame(&mut frame)? {
                return reader.finish();
            }
            if in_tx.send(frame).is_err() {
                return Ok(());
            }
        }
    });
    let write_thread = std::thread::spawn(move || -> Result<()> {
        let mut writer = writer;
        for frame in out_rx {
            writer.write_frame(&frame)?;
        }
        writer.finish()
    });

    let bar = if o.quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(expected_frames as u64)
    };
    bar.set_style(
        ProgressStyle::with_template("{bar:40} {pos}/{len} frames  {per_sec}  eta {eta}")
            .expect("valid fixed progress template"),
    );
    let mut processor = FrameProcessor::new(width as usize, height as usize, map.chain);
    let mut output = vec![0; frame_bytes];
    let mut frames = 0usize;
    let mut primary_error = None;
    while let Ok(input) = in_rx.recv() {
        let effect = modulator.effect_for_frame(frames);
        processor.process(&effect, frames, &input, &mut output);
        if let Err(error) = out_tx.send(output.clone()) {
            primary_error =
                Some(anyhow::Error::new(error).context("encoder stopped accepting frames"));
            break;
        }
        frames += 1;
        bar.inc(1);
    }
    drop(out_tx);
    drop(in_rx);
    bar.finish_and_clear();

    let reader_result = join_worker(read_thread, "decoder");
    let writer_result = join_worker(write_thread, "encoder");
    writer_result?;
    reader_result?;
    if let Some(error) = primary_error {
        return Err(error);
    }
    if frames == 0 {
        bail!("no frames were decoded from {}", o.video.display());
    }
    Ok(RenderSummary {
        frames,
        width,
        height,
        fps: info.fps,
    })
}
