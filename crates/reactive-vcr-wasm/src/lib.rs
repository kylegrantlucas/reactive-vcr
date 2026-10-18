//! C-ABI exports of the reactive-vcr core for `wasm32-unknown-unknown`, driving the
//! showcase site's live demo.
//!
//! No bindings generator: all buffers are Rust-owned and JavaScript reads and
//! writes them through views on the module's linear memory.
//!
//! 1. Text crosses the boundary through one shared byte buffer: JS writes UTF-8
//!    into `rvcr_text_buffer(len)`; Rust writes replies there and returns their
//!    length. Failures return `-n`, with the `n`-byte error message in the buffer.
//! 2. `rvcr_mapping_files(len)` lists the preset paths a mapping reads. JS
//!    fetches them and passes `{"toml": ..., "files": {path: text}}` to
//!    `rvcr_set_mapping(len)`.
//! 3. JS writes mono samples at 48 kHz into `rvcr_audio_buffer(count)`, then
//!    `rvcr_build(fps)` analyzes them and schedules every frame for the mapping.
//! 4. JS writes an RGBA frame into `rvcr_frame_buffer(w, h)`, and
//!    `rvcr_process(w, h, frame)` renders that scheduled frame into
//!    `rvcr_output()` (same size, RGBA).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use reactive_vcr::audio::features::{FeatureParams, SAMPLE_RATE, extract, frames_for};
use reactive_vcr::effect::FrameProcessor;
use reactive_vcr::mapping::{self, CompiledMapping, MappingFile};
use reactive_vcr::modulate::Schedule;
use reactive_vcr::params::ParamCatalog;

struct Engine {
    catalog: ParamCatalog,
    mapping: Option<CompiledMapping>,
    samples: Vec<f32>,
    schedule: Option<Schedule>,
    processor: Option<FrameProcessor>,
    /// RGBA frames shared with JS.
    input: Vec<u8>,
    output: Vec<u8>,
    /// RGB24 frames for the core processor.
    rgb_in: Vec<u8>,
    rgb_out: Vec<u8>,
    text: Vec<u8>,
}

impl Engine {
    fn new() -> Self {
        Self {
            catalog: ParamCatalog::new(),
            mapping: None,
            samples: Vec::new(),
            schedule: None,
            processor: None,
            input: Vec::new(),
            output: Vec::new(),
            rgb_in: Vec::new(),
            rgb_out: Vec::new(),
            text: Vec::new(),
        }
    }

    fn text(&self, len: u32) -> Result<&str> {
        let bytes = &self.text[..(len as usize).min(self.text.len())];
        std::str::from_utf8(bytes).context("text is not UTF-8")
    }

    fn write_text(&mut self, s: &str) -> i32 {
        self.text.clear();
        self.text.extend_from_slice(s.as_bytes());
        i32::try_from(self.text.len()).unwrap_or(i32::MAX)
    }

    /// A non-negative result, or `-n` with the error text in the text buffer.
    fn reply(&mut self, result: Result<i32>) -> i32 {
        match result {
            Ok(value) => value,
            Err(error) => -self.write_text(&format!("{error:#}")),
        }
    }
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::new());
}

fn with<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    ENGINE.with(|e| f(&mut e.borrow_mut()))
}

fn rgba_len(width: u32, height: u32) -> usize {
    width as usize * height as usize * 4
}

/// Shared text buffer, resized to `len` bytes.
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_text_buffer(len: u32) -> *mut u8 {
    with(|e| {
        e.text.resize(len as usize, 0);
        e.text.as_mut_ptr()
    })
}

/// Pointer to the shared text buffer (valid until the next call that writes text).
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_text_ptr() -> *const u8 {
    with(|e| e.text.as_ptr())
}

/// Writes the JSON array of preset paths that the mapping TOML in the first
/// `len` text bytes reads, exactly as written in the mapping; returns its length.
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_mapping_files(len: u32) -> i32 {
    with(|e| {
        let result = e.text(len).and_then(|toml| {
            let file = MappingFile::parse(toml).context("parsing mapping")?;
            let files: Vec<&PathBuf> = [
                file.base.as_ref().and_then(|base| base.preset.as_ref()),
                file.bender
                    .as_ref()
                    .and_then(|bender| bender.preset.as_ref()),
            ]
            .into_iter()
            .flatten()
            .collect();
            Ok(serde_json::to_string(&files)?)
        });
        match result {
            Ok(json) => e.write_text(&json),
            Err(error) => e.reply(Err(error)),
        }
    })
}

#[derive(serde::Deserialize)]
struct MappingInput {
    toml: String,
    /// Preset text keyed by the path written in the mapping.
    #[serde(default)]
    files: BTreeMap<String, String>,
}

/// Compiles `{"toml", "files"}` JSON from the first `len` text bytes as the
/// current mapping. Returns 0; call [`rvcr_build`] before processing again.
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_set_mapping(len: u32) -> i32 {
    with(|e| {
        let result = e.text(len).and_then(|json| {
            let input: MappingInput = serde_json::from_str(json).context("mapping input")?;
            let file = MappingFile::parse(&input.toml).context("parsing mapping")?;
            mapping::compile_with(file, Path::new(""), &e.catalog, |path| {
                path.to_str()
                    .and_then(|path| input.files.get(path))
                    .cloned()
                    .ok_or_else(|| anyhow!("file not supplied"))
            })
        });
        match result {
            Ok(compiled) => {
                e.mapping = Some(compiled);
                e.schedule = None;
                0
            }
            Err(error) => e.reply(Err(error)),
        }
    })
}

/// Buffer for `count` mono samples at 48 kHz; JS fills it before [`rvcr_build`].
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_audio_buffer(count: u32) -> *mut f32 {
    with(|e| {
        e.samples.clear();
        e.samples.resize(count as usize, 0.0);
        e.schedule = None;
        e.samples.as_mut_ptr()
    })
}

/// Analyzes the audio buffer at `fps` and schedules every frame of the current
/// mapping, as `render` does. Returns the number of frames.
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_build(fps: f64) -> i32 {
    with(|e| {
        let result = (|| {
            let Some(map) = &e.mapping else {
                bail!("no mapping loaded");
            };
            if !(fps.is_finite() && fps > 0.0) {
                bail!("frame rate must be positive, got {fps}");
            }
            let frames = frames_for(e.samples.len(), SAMPLE_RATE, fps, 0.0, None);
            if frames == 0 {
                bail!("the audio is shorter than one frame");
            }
            let features = extract(
                &e.samples,
                SAMPLE_RATE,
                &FeatureParams {
                    fps,
                    frames,
                    start_s: 0.0,
                    bands: map.bands.iter().map(|(_, range)| *range).collect(),
                    normalize: map.normalize,
                    onset_sensitivity: map.onset_sensitivity,
                },
            );
            let schedule = Schedule::build(&e.catalog, map, &features, fps);
            e.schedule = Some(schedule);
            Ok(i32::try_from(frames).unwrap_or(i32::MAX))
        })();
        e.reply(result)
    })
}

/// Input buffer for a `width`×`height` RGBA frame (reallocated on size change).
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_frame_buffer(width: u32, height: u32) -> *mut u8 {
    with(|e| {
        let len = rgba_len(width, height);
        e.input.resize(len, 0);
        e.output.resize(len, 255);
        e.input.as_mut_ptr()
    })
}

/// Rendered RGBA output of the last [`rvcr_process`].
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_output() -> *const u8 {
    with(|e| e.output.as_ptr())
}

/// Applies the effect scheduled for `frame` to the input buffer. Frames past the
/// end hold the last frame's settings. Returns 0; 1 if the buffers were not
/// sized for `width`×`height` by [`rvcr_frame_buffer`]; 2 before [`rvcr_build`].
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_process(width: u32, height: u32, frame: u32) -> i32 {
    with(|e| {
        let len = rgba_len(width, height);
        if width == 0 || height == 0 || e.input.len() != len || e.output.len() != len {
            return 1;
        }
        let Engine {
            catalog,
            schedule: Some(schedule),
            processor,
            input,
            output,
            rgb_in,
            rgb_out,
            ..
        } = e
        else {
            return 2;
        };
        let (w, h, frame) = (width as usize, height as usize, frame as usize);
        let chain = schedule.chain();
        let processor = match processor {
            Some(current) if current.fits(w, h, chain) => current,
            _ => processor.insert(FrameProcessor::new(w, h, chain)),
        };
        let rgb_len = w * h * 3;
        rgb_in.resize(rgb_len, 0);
        rgb_out.resize(rgb_len, 0);
        let (rgba, _) = input.as_chunks::<4>();
        let (rgb, _) = rgb_in.as_chunks_mut::<3>();
        for (&[r, g, b, _], dst) in rgba.iter().zip(rgb) {
            *dst = [r, g, b];
        }
        let effect = schedule.effect(catalog, frame);
        processor.process(&effect, frame, rgb_in, rgb_out);
        let (rgb, _) = rgb_out.as_chunks::<3>();
        let (rgba, _) = output.as_chunks_mut::<4>();
        for (&[r, g, b], dst) in rgb.iter().zip(rgba) {
            *dst = [r, g, b, 255];
        }
        0
    })
}

/// Forgets temporal effect state, e.g. after a seek, loop, or source change.
#[unsafe(no_mangle)]
pub extern "C" fn rvcr_reset() {
    with(|e| e.processor = None);
}
