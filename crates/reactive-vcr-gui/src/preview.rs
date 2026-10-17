//! Frame state shared between the GStreamer streaming thread and the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use eframe::egui;
use parking_lot::{Mutex, RwLock};
use reactive_vcr::effect::FrameProcessor;
use reactive_vcr_gst::Program;
use reactive_vcr_gst::gst::ClockTime;
use reactive_vcr_gst::preview::PreviewFrame;

/// Effect processor and output scratch, reused across frames.
#[derive(Default)]
struct Scratch {
    processor: Option<FrameProcessor>,
    rgb: Vec<u8>,
}

/// The last decoded frame plus the program applied to it, re-rendered on demand.
pub struct PreviewShared {
    /// The effect program; `None` shows clean frames until analysis finishes.
    pub program: RwLock<Option<Arc<Program>>>,
    pub bypass: AtomicBool,
    /// Frames from pipelines of other generations are stale and ignored.
    generation: AtomicU64,
    frame: Mutex<Option<PreviewFrame>>,
    scratch: Mutex<Scratch>,
    texture: Mutex<Option<egui::TextureHandle>>,
    /// Stream time of the frame in the texture.
    shown: Mutex<Option<ClockTime>>,
    ctx: egui::Context,
}

impl PreviewShared {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            program: RwLock::new(None),
            bypass: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            frame: Mutex::new(None),
            scratch: Mutex::new(Scratch::default()),
            texture: Mutex::new(None),
            shown: Mutex::new(None),
            ctx,
        }
    }

    /// Starts accepting frames from pipeline `generation` only, and forgets the stored frame.
    /// The texture stays up until a new frame replaces it unless `clear_texture` is set.
    pub fn reset(&self, generation: u64, clear_texture: bool) {
        self.generation.store(generation, Ordering::SeqCst);
        *self.frame.lock() = None;
        self.scratch.lock().processor = None;
        if clear_texture {
            *self.texture.lock() = None;
            *self.shown.lock() = None;
        }
    }

    /// Appsink callback: stores the frame and renders it.
    pub fn on_frame(&self, generation: u64, frame: PreviewFrame) {
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        *self.frame.lock() = Some(frame);
        self.render();
    }

    pub fn set_program(&self, program: Option<Arc<Program>>) {
        *self.program.write() = program;
    }

    pub fn texture(&self) -> Option<egui::TextureHandle> {
        self.texture.lock().clone()
    }

    /// Stream time of the frame on screen.
    pub fn shown(&self) -> Option<ClockTime> {
        *self.shown.lock()
    }

    /// Applies the effect to the stored frame and uploads it as the preview texture.
    pub fn render(&self) {
        let frame = self.frame.lock();
        let Some(f) = frame.as_ref() else {
            return;
        };
        let row = f.width * 3;
        let program = if self.bypass.load(Ordering::Relaxed) {
            None
        } else {
            self.program.read().clone()
        };
        let mut scratch = self.scratch.lock();
        let Scratch { processor, rgb } = &mut *scratch;
        rgb.resize(row * f.height, 0);
        match program {
            Some(program) => {
                let (index, effect) = program.effect_at(f.stream_time);
                let chain = program.schedule.chain();
                let processor = match processor {
                    Some(current) if current.fits(f.width, f.height, chain) => current,
                    _ => processor.insert(FrameProcessor::new(f.width, f.height, chain)),
                };
                processor.process_strided(&effect, index, &f.data, f.stride, rgb, row);
            }
            None => {
                for (dst, src) in rgb.chunks_exact_mut(row).zip(f.data.chunks(f.stride)) {
                    dst.copy_from_slice(&src[..row]);
                }
            }
        }
        let image = egui::ColorImage::from_rgb([f.width, f.height], rgb);
        let time = f.stream_time;
        drop(scratch);
        drop(frame);

        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            ..Default::default()
        };
        let mut texture = self.texture.lock();
        match texture.as_mut() {
            Some(t) => t.set(image, options),
            None => *texture = Some(self.ctx.load_texture("preview", image, options)),
        }
        drop(texture);
        *self.shown.lock() = Some(time);
        self.ctx.request_repaint();
    }
}
