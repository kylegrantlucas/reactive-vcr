//! Main window: inputs, mapping editor, preview with meters, transport and render queue.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::{VideoInfo, select_timeline};
use reactive_vcr_gst::AudioMode;
use reactive_vcr_gst::decode::decode_mono_f32;
use reactive_vcr_gst::gst::ClockTime;
use reactive_vcr_gst::preview::{PreviewEvent, PreviewOptions, PreviewPipeline};
use reactive_vcr_gst::probe::probe;

use crate::analysis::{Analysis, ProgramWorker, Task};
use crate::editor::{self, MappingDoc};
use crate::meters;
use crate::preview::PreviewShared;
use crate::render::{
    RenderInputs, RenderPanel, audio_mode_combo, audio_mode_from_name, audio_mode_name,
    effective_audio_mode,
};

const MAPPING_KEY: &str = "vcr.mapping";
const MAPPING_PATH_KEY: &str = "vcr.mapping_path";
const PREVIEW_LINES_KEY: &str = "vcr.preview_lines";
const VOLUME_KEY: &str = "vcr.volume";
const AUDIO_MODE_KEY: &str = "vcr.audio_mode";
const RENDER_AUDIO_MODE_KEY: &str = "vcr.render_audio_mode";
const CODEC_KEY: &str = "vcr.codec";
const QUANTIZER_KEY: &str = "vcr.quantizer";

const DEFAULT_PREVIEW_LINES: u32 = 480;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "ts", "mts", "m2ts", "wmv", "flv",
    "3gp", "ogv", "dv", "mxf",
];
const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "flac", "aac", "m4a", "ogg", "oga", "opus", "aif", "aiff", "aifc", "wma", "caf",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Video,
    Audio,
    Mapping,
}

fn classify(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext == "toml" {
        Some(FileKind::Mapping)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(FileKind::Video)
    } else if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        Some(FileKind::Audio)
    } else {
        None
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// An input file and its background probe or decode.
struct Input<T> {
    path: PathBuf,
    task: Task<T>,
}

/// Paths given on the command line.
pub struct Startup {
    pub video: Option<PathBuf>,
    pub audio: Option<PathBuf>,
    pub mapping: Option<PathBuf>,
}

pub struct VcrApp {
    catalog: Arc<ParamCatalog>,
    shared: Arc<PreviewShared>,
    doc: MappingDoc,
    video: Option<Input<VideoInfo>>,
    audio: Option<Input<Arc<Vec<f32>>>>,
    /// The video's own soundtrack drives the effect.
    audio_from_video: bool,
    worker: Option<ProgramWorker>,
    /// Generation of the newest program request, and of the newest finished build.
    requested: u64,
    built: u64,
    analysis: Option<Analysis>,
    preview: Option<PreviewPipeline>,
    pending_preview: Option<Task<PreviewPipeline>>,
    preview_generation: u64,
    /// Whether playback is (or, while the preview loads, will be) running.
    playing: bool,
    preview_lines: u32,
    volume: f64,
    audio_mode: AudioMode,
    seek_drag: Option<f64>,
    error: Option<String>,
    render: RenderPanel,
}

impl VcrApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        let mut error = None;
        let mut doc = None;
        let mut render = RenderPanel::default();
        let mut preview_lines = DEFAULT_PREVIEW_LINES;
        let mut volume = 1.0;
        let mut audio_mode = AudioMode::Mix;
        if let Some(storage) = cc.storage {
            if startup.mapping.is_none()
                && let Some(text) = eframe::get_value::<String>(storage, MAPPING_KEY)
            {
                let path = eframe::get_value::<Option<String>>(storage, MAPPING_PATH_KEY)
                    .flatten()
                    .map(PathBuf::from);
                match MappingDoc::restore(&text, path) {
                    Ok(restored) => doc = Some(restored),
                    Err(e) => error = Some(format!("{e:#}")),
                }
            }
            if let Some(lines) = eframe::get_value::<u32>(storage, PREVIEW_LINES_KEY) {
                preview_lines = lines.clamp(120, 2160);
            }
            if let Some(v) = eframe::get_value::<f64>(storage, VOLUME_KEY) {
                volume = v.clamp(0.0, 2.0);
            }
            let mode = |key| {
                eframe::get_value::<String>(storage, key).and_then(|n| audio_mode_from_name(&n))
            };
            if let Some(mode) = mode(AUDIO_MODE_KEY) {
                audio_mode = mode;
            }
            if let Some(mode) = mode(RENDER_AUDIO_MODE_KEY) {
                render.audio_mode = mode;
            }
            if let Some(codec) = eframe::get_value::<String>(storage, CODEC_KEY) {
                render.h264 = codec != "ffv1";
            }
            if let Some(q) = eframe::get_value::<u32>(storage, QUANTIZER_KEY) {
                render.quantizer = q.min(51);
            }
        }
        if let Some(path) = &startup.mapping {
            match MappingDoc::open(path) {
                Ok(opened) => doc = Some(opened),
                Err(e) => error = Some(format!("{e:#}")),
            }
        }

        let mut app = Self {
            catalog: Arc::new(ParamCatalog::new()),
            shared: Arc::new(PreviewShared::new(cc.egui_ctx.clone())),
            doc: doc.unwrap_or_else(MappingDoc::empty),
            video: None,
            audio: None,
            audio_from_video: false,
            worker: None,
            requested: 0,
            built: 0,
            analysis: None,
            preview: None,
            pending_preview: None,
            preview_generation: 0,
            playing: false,
            preview_lines,
            volume,
            audio_mode,
            seek_drag: None,
            error,
            render,
        };
        app.mapping_changed();
        let ctx = &cc.egui_ctx;
        if let Some(path) = startup.video {
            app.open_video(ctx, path);
        }
        if let Some(path) = startup.audio {
            app.open_audio(ctx, path);
        }
        app
    }

    // ---- inputs -------------------------------------------------------------------------

    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        match classify(&path) {
            Some(FileKind::Video) => self.open_video(ctx, path),
            Some(FileKind::Audio) => self.open_audio(ctx, path),
            Some(FileKind::Mapping) => self.open_mapping(&path),
            None => {
                self.error = Some(format!(
                    "{}: unsupported file type; use a video, an audio file, or a .toml mapping",
                    file_name(&path)
                ));
            }
        }
    }

    fn open_video(&mut self, ctx: &egui::Context, path: PathBuf) {
        let probed = path.clone();
        self.video = Some(Input {
            path: path.clone(),
            task: Task::spawn(ctx, move || probe(&probed)),
        });
        self.invalidate_analysis();
        self.stop_preview(true);
        if self.audio_from_video {
            self.load_audio(ctx, path);
        }
    }

    fn open_audio(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.audio_from_video = false;
        self.load_audio(ctx, path);
    }

    fn load_audio(&mut self, ctx: &egui::Context, path: PathBuf) {
        let decoded = path.clone();
        self.audio = Some(Input {
            path,
            task: Task::spawn(ctx, move || decode_mono_f32(&decoded).map(Arc::new)),
        });
        self.invalidate_analysis();
        // The preview plays the driving audio, so it needs the new file.
        let position = self.position();
        let playing = self.playing;
        self.rebuild_preview(ctx, position, playing, false);
    }

    fn set_audio_from_video(&mut self, ctx: &egui::Context, on: bool) {
        self.audio_from_video = on;
        if on {
            if let Some(video) = &self.video {
                let path = video.path.clone();
                self.load_audio(ctx, path);
            }
        } else {
            // The soundtrack no longer drives anything; ask for a song again.
            self.audio = None;
            self.invalidate_analysis();
            self.stop_preview(true);
        }
    }

    fn open_mapping(&mut self, path: &Path) {
        match MappingDoc::open(path) {
            Ok(doc) => {
                self.doc = doc;
                self.mapping_changed();
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn save_mapping(&mut self) {
        let name = self
            .doc
            .path
            .as_deref()
            .map_or_else(|| "mapping.toml".into(), file_name);
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Mapping", &["toml"])
            .set_file_name(name);
        if let Some(dir) = self.doc.path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("toml");
        }
        match self.doc.save_as(&path) {
            Ok(()) => self.mapping_changed(),
            Err(e) => self.error = Some(format!("saving mapping: {e:#}")),
        }
    }

    // ---- analysis -----------------------------------------------------------------------

    fn invalidate_analysis(&mut self) {
        self.worker = None;
        self.analysis = None;
        self.shared.set_program(None);
    }

    /// Recompiles the edited mapping and, if it compiles, rebuilds the program.
    fn mapping_changed(&mut self) {
        if self.doc.recompile(&self.catalog) {
            self.request_program();
        }
    }

    fn request_program(&mut self) {
        if let (Some(worker), Some(map)) = (&self.worker, &self.doc.compiled) {
            self.requested += 1;
            worker.request(self.requested, map.clone());
        }
    }

    fn start_worker(&mut self, ctx: &egui::Context) {
        if self.worker.is_some() {
            return;
        }
        let (Some(video), Some(audio)) = (&self.video, &self.audio) else {
            return;
        };
        let (Some(info), Some(samples)) = (video.task.ready(), audio.task.ready()) else {
            return;
        };
        match select_timeline(info, None, None, None) {
            Ok(selection) => {
                self.worker = Some(ProgramWorker::new(
                    ctx,
                    self.catalog.clone(),
                    samples.clone(),
                    selection,
                ));
                self.request_program();
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn is_analyzing(&self) -> bool {
        self.video.as_ref().is_some_and(|v| v.task.is_running())
            || self.audio.as_ref().is_some_and(|a| a.task.is_running())
            || (self.worker.is_some() && self.built < self.requested)
    }

    /// Collects finished background work.
    fn poll(&mut self, ctx: &egui::Context) {
        let mut video_done = false;
        if let Some(video) = &mut self.video
            && video.task.poll()
        {
            match &video.task {
                Task::Failed(e) => {
                    self.error = Some(format!("opening {}: {e}", file_name(&video.path)));
                }
                _ => video_done = true,
            }
        }
        let mut audio_done = false;
        if let Some(audio) = &mut self.audio
            && audio.task.poll()
        {
            match &audio.task {
                Task::Failed(e) => {
                    self.error = Some(format!("decoding {}: {e}", file_name(&audio.path)));
                }
                _ => audio_done = true,
            }
        }
        if video_done {
            self.rebuild_preview(ctx, None, false, true);
        }
        if video_done || audio_done {
            self.start_worker(ctx);
        }

        if let Some((generation, result)) = self.worker.as_ref().and_then(ProgramWorker::poll) {
            self.built = generation;
            match result {
                Ok(analysis) => {
                    self.shared.set_program(Some(analysis.program.clone()));
                    self.analysis = Some(analysis);
                    if self.preview.is_none() || !self.playing {
                        self.shared.render();
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }

        if self.pending_preview.as_mut().is_some_and(Task::poll) {
            match self.pending_preview.take() {
                Some(Task::Ready(preview)) => self.preview = Some(preview),
                Some(Task::Failed(e)) => {
                    self.playing = false;
                    self.error = Some(format!("preview: {e}"));
                }
                _ => {}
            }
        }

        if let Some(preview) = &self.preview {
            let mut events = Vec::new();
            while let Some(event) = preview.poll() {
                events.push(event);
            }
            for event in events {
                match event {
                    PreviewEvent::Ended => {
                        if let Err(e) = preview.seek(preview.start()) {
                            self.error = Some(format!("looping: {e:#}"));
                        }
                    }
                    PreviewEvent::Error(e) => self.error = Some(format!("preview: {e}")),
                }
            }
            if self.playing {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    // ---- preview ------------------------------------------------------------------------

    fn stop_preview(&mut self, clear_texture: bool) {
        self.preview = None;
        self.pending_preview = None;
        self.playing = false;
        self.preview_generation += 1;
        self.shared.reset(self.preview_generation, clear_texture);
    }

    /// (Re)builds the preview on a thread, resuming at `position`.
    fn rebuild_preview(
        &mut self,
        ctx: &egui::Context,
        position: Option<ClockTime>,
        play: bool,
        clear_texture: bool,
    ) {
        self.stop_preview(clear_texture);
        let (Some(video), Some(audio)) = (&self.video, &self.audio) else {
            return;
        };
        let Some(info) = video.task.ready() else {
            return;
        };
        let scale = (f64::from(self.preview_lines) / f64::from(info.height.max(1))).min(1.0);
        let selection = match select_timeline(info, Some(scale), None, None) {
            Ok(selection) => selection,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        let options = PreviewOptions {
            video: video.path.clone(),
            audio: audio.path.clone(),
            audio_mode: effective_audio_mode(self.audio_mode, &video.path, &audio.path),
            has_original_audio: info.has_audio,
            selection,
        };
        let generation = self.preview_generation;
        let shared = self.shared.clone();
        let volume = self.volume;
        self.playing = play;
        self.pending_preview = Some(Task::spawn(ctx, move || {
            let preview =
                PreviewPipeline::new(&options, move |frame| shared.on_frame(generation, frame))?;
            preview.set_volume(volume);
            if let Some(position) = position.filter(|p| *p > preview.start()) {
                preview.seek(position)?;
            }
            if play {
                preview.play()?;
            }
            Ok(preview)
        }));
    }

    fn position(&self) -> Option<ClockTime> {
        self.preview.as_ref().and_then(PreviewPipeline::position)
    }

    fn set_playing(&mut self, play: bool) {
        let Some(preview) = &self.preview else {
            return;
        };
        let result = if play {
            preview.play()
        } else {
            preview.pause()
        };
        match result {
            Ok(()) => self.playing = play,
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    // ---- panels -------------------------------------------------------------------------

    fn top_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open video…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video", VIDEO_EXTENSIONS)
                    .add_filter("All files", &["*"])
                    .pick_file()
            {
                self.open_video(&ctx, path);
            }
            if ui.button("Open audio…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Audio", AUDIO_EXTENSIONS)
                    .add_filter("All files", &["*"])
                    .pick_file()
            {
                self.open_audio(&ctx, path);
            }
            if ui.button("Open mapping…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Mapping", &["toml"])
                    .pick_file()
            {
                self.open_mapping(&path);
            }
            if ui.button("Save mapping…").clicked() {
                self.save_mapping();
            }
            if ui
                .button("Reset")
                .on_hover_text("Start over with an empty mapping")
                .clicked()
            {
                self.doc = MappingDoc::empty();
                self.mapping_changed();
            }
            ui.separator();
            let mut from_video = self.audio_from_video;
            if ui
                .add_enabled(
                    self.video.is_some(),
                    egui::Checkbox::new(&mut from_video, "Video's soundtrack drives the effect"),
                )
                .on_hover_text("Use the video file's own audio instead of a separate song")
                .changed()
            {
                self.set_audio_from_video(&ctx, from_video);
            }
            if let Some(err) = &self.error {
                ui.separator();
                ui.colored_label(ui.visuals().error_fg_color, err);
                if ui.small_button("×").clicked() {
                    self.error = None;
                }
            }
        });
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            let loading = self.pending_preview.is_some();
            let label = if self.playing { "⏸" } else { "▶" };
            if ui
                .add_enabled(self.preview.is_some(), egui::Button::new(label))
                .on_hover_text("Play/pause (space)")
                .clicked()
            {
                self.set_playing(!self.playing);
            }
            if loading {
                ui.add(egui::Spinner::new());
            }

            let duration = self
                .video
                .as_ref()
                .and_then(|v| v.task.ready())
                .map_or(0.0, |info| info.duration_s);
            let position = self.position();
            let pos = self
                .seek_drag
                .unwrap_or_else(|| position.map_or(0.0, seconds));
            let mut value = pos;
            let response = ui.add_enabled(
                self.preview.is_some() && duration > 0.0,
                egui::Slider::new(&mut value, 0.0..=duration.max(f64::EPSILON)).show_value(false),
            );
            if response.dragged() || response.changed() {
                self.seek_drag = Some(value);
            }
            if (response.drag_stopped() || (response.changed() && !response.dragged()))
                && let (Some(target), Some(p)) = (self.seek_drag.take(), &self.preview)
                && let Err(e) = p.seek(ClockTime::from_nseconds((target * 1e9) as u64))
            {
                self.error = Some(format!("{e:#}"));
            }
            ui.monospace(format!("{} / {}", timestamp(value), timestamp(duration)));

            ui.separator();
            ui.label("Volume");
            if ui
                .add(egui::Slider::new(&mut self.volume, 0.0..=2.0).show_value(false))
                .changed()
                && let Some(p) = &self.preview
            {
                p.set_volume(self.volume);
            }
            ui.label("Audio");
            if audio_mode_combo(ui, "preview-audio", &mut self.audio_mode) {
                let playing = self.playing;
                self.rebuild_preview(&ctx, position, playing, false);
            }

            ui.separator();
            let mut bypass = self.shared.bypass.load(Ordering::Relaxed);
            if ui
                .checkbox(&mut bypass, "Bypass")
                .on_hover_text("Show the clean frame")
                .changed()
            {
                self.shared.bypass.store(bypass, Ordering::Relaxed);
                self.shared.render();
            }
            ui.label("Preview");
            let mut lines = self.preview_lines;
            let response = ui
                .add(
                    egui::Slider::new(&mut lines, 120..=1080)
                        .step_by(2.0)
                        .suffix(" lines"),
                )
                .on_hover_text("Preview processing height; renders use the Scale setting");
            self.preview_lines = lines;
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                let playing = self.playing;
                self.rebuild_preview(&ctx, position, playing, false);
            }
        });
    }

    fn inputs_summary(&self, ui: &mut egui::Ui) {
        ui.heading("Inputs");
        egui::Grid::new("inputs")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Video");
                match &self.video {
                    Some(video) => {
                        ui.label(file_name(&video.path))
                            .on_hover_text(video.path.display().to_string());
                        ui.end_row();
                        if let Some(info) = video.task.ready() {
                            ui.label("");
                            ui.weak(format!(
                                "{}×{} · {:.2} fps · {} {}",
                                info.width,
                                info.height,
                                info.fps(),
                                timestamp(info.duration_s),
                                if info.has_audio {
                                    "· sound"
                                } else {
                                    "· silent"
                                },
                            ));
                        }
                    }
                    None => {
                        ui.weak("none");
                    }
                }
                ui.end_row();
                ui.label("Audio");
                match &self.audio {
                    Some(_) if self.audio_from_video => {
                        ui.label("video soundtrack");
                    }
                    Some(audio) => {
                        ui.label(file_name(&audio.path))
                            .on_hover_text(audio.path.display().to_string());
                    }
                    None => {
                        ui.weak("none");
                    }
                }
                ui.end_row();
                if let Some(samples) = self.audio.as_ref().and_then(|a| a.task.ready()) {
                    ui.label("");
                    ui.weak(timestamp(samples.len() as f64 / 48_000.0));
                    ui.end_row();
                }
            });
    }

    fn instructions(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).max(16.0));
            ui.heading("Drop a video and a song anywhere in this window");
            ui.add_space(12.0);
            ui.label("The song's loudness, frequency bands and hits drive the VHS look.");
            ui.add_space(16.0);
            let check = |done: bool| if done { "✔" } else { "○" };
            match &self.video {
                Some(video) => {
                    ui.label(format!("{} Video: {}", check(true), file_name(&video.path)))
                }
                None => ui.label(format!(
                    "{} Video: drop a clip (mp4, mov, mkv…) or use Open video…",
                    check(false)
                )),
            };
            match &self.audio {
                Some(audio) => {
                    ui.label(format!("{} Audio: {}", check(true), file_name(&audio.path)))
                }
                None => ui.label(format!(
                    "{} Audio: drop a song (wav, mp3, flac…) or use Open audio…",
                    check(false)
                )),
            };
            if self.video.is_some() && self.audio.is_none() {
                let mut from_video = false;
                if ui
                    .checkbox(
                        &mut from_video,
                        "or let the video's own soundtrack drive it",
                    )
                    .changed()
                {
                    self.set_audio_from_video(&ctx, true);
                }
            }
            ui.label(format!(
                "{} Mapping: {} — edit it on the left, or drop a .toml",
                check(true),
                self.doc.title()
            ));
        });
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        if self.video.is_none() || self.audio.is_none() {
            self.instructions(ui);
            return;
        }
        let status = if self.video.as_ref().is_some_and(|v| v.task.is_running()) {
            Some("analyzing… probing the video")
        } else if self.audio.as_ref().is_some_and(|a| a.task.is_running()) {
            Some("analyzing… decoding the audio")
        } else if self.is_analyzing() {
            Some("analyzing… building the effect")
        } else if self.pending_preview.is_some() {
            Some("loading the preview…")
        } else {
            None
        };
        if let Some(status) = status {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(status);
            });
        }
        if let (Some(analysis), Some(time)) = (&self.analysis, self.shared.shown()) {
            egui::Panel::bottom("meters")
                .resizable(false)
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    let (frame, effect) = analysis.program.effect_at(time);
                    meters::show(ui, &self.catalog, analysis, frame, &effect);
                    ui.add_space(2.0);
                });
        }
        match self.shared.texture() {
            Some(texture) => {
                let avail = ui.available_size();
                let size = texture.size_vec2();
                let scale = (avail.x / size.x).min(avail.y / size.y).max(0.0);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(&texture).fit_to_exact_size(size * scale));
                });
            }
            None => {
                ui.centered_and_justified(|ui| ui.weak("Waiting for the first frame…"));
            }
        }
    }

    fn render_panel(&mut self, ui: &mut egui::Ui) {
        self.inputs_summary(ui);
        ui.separator();
        let inputs = render_inputs(
            self.video.as_ref(),
            self.audio.as_ref(),
            &self.doc,
            self.analysis.as_ref(),
            self.built == self.requested,
        );
        if let Some(e) = self.render.show(ui, inputs) {
            self.error = Some(e);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for path in dropped {
            self.open_path(ctx, path);
        }
        let hovered: Vec<Option<PathBuf>> =
            ctx.input(|i| i.raw.hovered_files.iter().map(|f| f.path.clone()).collect());
        if hovered.is_empty() {
            return;
        }
        let lines: Vec<String> = hovered
            .iter()
            .map(|path| match path {
                Some(path) => {
                    let what = match classify(path) {
                        Some(FileKind::Video) => "video",
                        Some(FileKind::Audio) => "driving audio",
                        Some(FileKind::Mapping) => "mapping",
                        None => "unsupported",
                    };
                    format!("{what}: {}", file_name(path))
                }
                None => "file".to_string(),
            })
            .collect();
        let text = format!("Drop to open\n\n{}", lines.join("\n"));
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("drop-hint"),
        ));
        let rect = ctx.content_rect();
        painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(190));
        painter.rect_stroke(
            rect.shrink(12.0),
            8.0,
            egui::Stroke::new(2.0, ctx.global_style().visuals.selection.bg_fill),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(22.0),
            egui::Color32::WHITE,
        );
    }
}

/// Render inputs, or why rendering is unavailable.
fn render_inputs<'a>(
    video: Option<&'a Input<VideoInfo>>,
    audio: Option<&'a Input<Arc<Vec<f32>>>>,
    doc: &MappingDoc,
    analysis: Option<&Analysis>,
    up_to_date: bool,
) -> Result<RenderInputs<'a>, &'static str> {
    let video = video.ok_or("Open a video first")?;
    let info = video.task.ready().ok_or("Waiting for the video")?;
    let audio = audio.ok_or("Open the driving audio first")?;
    if audio.task.ready().is_none() {
        return Err("Waiting for the audio");
    }
    if doc.errors.is_some() {
        return Err("Fix the mapping errors first");
    }
    let analysis = analysis.ok_or("Analyzing…")?;
    if !up_to_date {
        return Err("Updating the effect…");
    }
    let mut protected = Vec::new();
    if let Some(path) = &doc.path {
        protected.push(("mapping", path.clone()));
    }
    if let Some(compiled) = &doc.compiled {
        protected.extend(
            compiled
                .preset_inputs()
                .into_iter()
                .map(|(label, path)| (label, path.to_path_buf())),
        );
    }
    Ok(RenderInputs {
        video: &video.path,
        audio: &audio.path,
        info,
        program: analysis.program.clone(),
        protected,
    })
}

impl eframe::App for VcrApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.handle_drops(&ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !ctx.egui_wants_keyboard_input() {
            self.set_playing(!self.playing);
        }

        egui::Panel::top("top").show(ui, |ui| self.top_panel(ui));
        egui::Panel::bottom("transport").show(ui, |ui| self.bottom_panel(ui));
        egui::Panel::left("mapping")
            .resizable(true)
            .default_size(380.0)
            .size_range(300.0..=640.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if editor::show(ui, &mut self.doc, &self.catalog) {
                        self.mapping_changed();
                    }
                });
            });
        egui::Panel::right("render")
            .resizable(true)
            .default_size(250.0)
            .size_range(220.0..=420.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.render_panel(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| self.central_panel(ui));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        match self.doc.file.to_toml() {
            Ok(text) => eframe::set_value(storage, MAPPING_KEY, &text),
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        let path = self.doc.path.as_ref().map(|p| p.display().to_string());
        eframe::set_value(storage, MAPPING_PATH_KEY, &path);
        eframe::set_value(storage, PREVIEW_LINES_KEY, &self.preview_lines);
        eframe::set_value(storage, VOLUME_KEY, &self.volume);
        eframe::set_value(storage, AUDIO_MODE_KEY, &audio_mode_name(self.audio_mode));
        eframe::set_value(
            storage,
            RENDER_AUDIO_MODE_KEY,
            &audio_mode_name(self.render.audio_mode),
        );
        eframe::set_value(
            storage,
            CODEC_KEY,
            &if self.render.h264 { "h264" } else { "ffv1" },
        );
        eframe::set_value(storage, QUANTIZER_KEY, &self.render.quantizer);
    }
}

fn seconds(t: ClockTime) -> f64 {
    t.nseconds() as f64 / 1e9
}

/// Formats seconds as `mm:ss.ff`.
fn timestamp(secs: f64) -> String {
    let centis = (secs.max(0.0) * 100.0).round() as u64;
    format!(
        "{:02}:{:02}.{:02}",
        centis / 6000,
        centis / 100 % 60,
        centis % 100
    )
}
