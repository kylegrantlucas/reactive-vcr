//! Render settings, the render button, and the job list.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use eframe::egui;
use reactive_vcr::timeline::{VideoInfo, select_timeline, validate_output_is_distinct};
use reactive_vcr_gst::render::{OutputCodec, RenderJob, RenderSettings, RenderStatus};
use reactive_vcr_gst::{AudioMode, Program};

use crate::analysis::Task;

/// Everything a render needs from the rest of the app.
pub struct RenderInputs<'a> {
    pub video: &'a Path,
    pub audio: &'a Path,
    pub info: &'a VideoInfo,
    pub program: Arc<Program>,
    /// Files the output must not overwrite besides the media: mapping and presets.
    pub protected: Vec<(&'static str, PathBuf)>,
}

pub struct RenderPanel {
    pub h264: bool,
    pub quantizer: u32,
    pub scale: f64,
    pub audio_mode: AudioMode,
    jobs: Vec<(PathBuf, Task<RenderJob>)>,
}

impl Default for RenderPanel {
    fn default() -> Self {
        Self {
            h264: true,
            quantizer: OutputCodec::DEFAULT_QUANTIZER,
            scale: 1.0,
            audio_mode: AudioMode::Mix,
            jobs: Vec::new(),
        }
    }
}

impl RenderPanel {
    fn codec(&self) -> OutputCodec {
        if self.h264 {
            OutputCodec::H264 {
                quantizer: self.quantizer,
            }
        } else {
            OutputCodec::Ffv1
        }
    }

    /// Draws the panel. `inputs` is `Err(reason)` while rendering is unavailable.
    /// Returns an error to show in the top bar.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        inputs: Result<RenderInputs<'_>, &str>,
    ) -> Option<String> {
        let mut error = None;
        ui.heading("Render");
        egui::Grid::new("render-settings")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Codec");
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(codec_label(self.h264))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.h264, true, codec_label(true));
                        ui.selectable_value(&mut self.h264, false, codec_label(false));
                    });
                ui.end_row();
                if self.h264 {
                    ui.label("Quantizer");
                    ui.add(egui::Slider::new(&mut self.quantizer, 0..=51))
                        .on_hover_text(
                            "x264 constant quantizer; lower is higher quality, 0 is lossless",
                        );
                    ui.end_row();
                }
                ui.label("Scale");
                ui.add(egui::Slider::new(&mut self.scale, 0.25..=1.0).fixed_decimals(2))
                    .on_hover_text("Output size relative to the source video");
                ui.end_row();
                ui.label("Audio");
                audio_mode_combo(ui, "render-audio", &mut self.audio_mode);
                ui.end_row();
            });

        let reason = inputs.as_ref().err().copied();
        let clicked = ui
            .add_enabled(reason.is_none(), egui::Button::new("Render…"))
            .on_disabled_hover_text(reason.unwrap_or(""))
            .clicked();
        if let Some(reason) = reason {
            ui.weak(reason);
        }
        if clicked
            && let Ok(inputs) = inputs
            && let Err(e) = self.start(ui.ctx(), inputs)
        {
            error = Some(format!("render: {e:#}"));
        }

        ui.separator();
        let mut busy = false;
        let mut cancel = None;
        for (index, (output, task)) in self.jobs.iter_mut().enumerate() {
            task.poll();
            let name = output.file_name().map_or_else(
                || output.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            ui.label(name).on_hover_text(output.display().to_string());
            match task {
                Task::Running(_) => {
                    busy = true;
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Starting…");
                    });
                }
                Task::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Failed: {e}"));
                }
                Task::Ready(job) => match job.status() {
                    RenderStatus::Running { progress } => {
                        busy = true;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::ProgressBar::new(progress as f32)
                                    .show_percentage()
                                    .desired_width(140.0),
                            );
                            if ui.button("Cancel").clicked() {
                                cancel = Some(index);
                            }
                        });
                    }
                    RenderStatus::Done => {
                        ui.label(format!("Done · {} frames", job.frames_rendered()));
                    }
                    RenderStatus::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, format!("Failed: {e}"));
                    }
                    RenderStatus::Cancelled => {
                        ui.label("Cancelled");
                    }
                },
            }
            ui.add_space(4.0);
        }
        if let Some(index) = cancel
            && let Task::Ready(job) = &self.jobs[index].1
        {
            job.cancel();
        }
        if !busy && !self.jobs.is_empty() && ui.small_button("Clear finished").clicked() {
            self.jobs.clear();
        }
        if busy {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        error
    }

    fn start(&mut self, ctx: &egui::Context, inputs: RenderInputs<'_>) -> Result<()> {
        let codec = self.codec();
        let ext = codec.extension();
        let stem = inputs
            .video
            .file_stem()
            .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
        let mut dialog = rfd::FileDialog::new()
            .add_filter(ext, &[ext])
            .set_file_name(format!("{stem}-vcr.{ext}"));
        if let Some(dir) = inputs.video.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut output) = dialog.save_file() else {
            return Ok(());
        };
        if output.extension().is_none() {
            output.set_extension(ext);
        }
        validate_output_is_distinct(&output, &[("video", inputs.video), ("audio", inputs.audio)])?;
        let protected: Vec<(&str, &Path)> = inputs
            .protected
            .iter()
            .map(|(label, path)| (*label, path.as_path()))
            .collect();
        validate_output_is_distinct(&output, &protected)?;
        let selection = select_timeline(inputs.info, Some(self.scale), None, None)?;
        let settings = RenderSettings {
            video: inputs.video.to_path_buf(),
            audio: inputs.audio.to_path_buf(),
            output: output.clone(),
            codec,
            audio_mode: effective_audio_mode(self.audio_mode, inputs.video, inputs.audio),
            selection,
            has_original_audio: inputs.info.has_audio,
            program: inputs.program,
        };
        self.jobs
            .push((output, Task::spawn(ctx, move || RenderJob::start(settings))));
        Ok(())
    }
}

fn codec_label(h264: bool) -> &'static str {
    if h264 { "H.264 MP4" } else { "FFV1 MKV" }
}

pub const AUDIO_MODES: [(AudioMode, &str, &str); 4] = [
    (
        AudioMode::Mix,
        "mix",
        "Driving audio plus the video's own sound",
    ),
    (
        AudioMode::Driving,
        "driving",
        "Only the audio that drives the effect",
    ),
    (
        AudioMode::Original,
        "original",
        "Only the video's own sound",
    ),
    (AudioMode::None, "none", "Silence"),
];

pub fn audio_mode_name(mode: AudioMode) -> &'static str {
    AUDIO_MODES
        .iter()
        .find(|(m, ..)| *m == mode)
        .map_or("mix", |(_, name, _)| name)
}

pub fn audio_mode_from_name(name: &str) -> Option<AudioMode> {
    AUDIO_MODES
        .iter()
        .find(|(_, n, _)| *n == name)
        .map(|(mode, ..)| *mode)
}

/// Audio mode combo box. Returns true when the mode changed.
pub fn audio_mode_combo(ui: &mut egui::Ui, id: &str, mode: &mut AudioMode) -> bool {
    let before = *mode;
    egui::ComboBox::from_id_salt(id)
        .selected_text(audio_mode_name(*mode))
        .show_ui(ui, |ui| {
            for (m, name, hint) in AUDIO_MODES {
                ui.selectable_value(mode, m, name).on_hover_text(hint);
            }
        });
    *mode != before
}

/// When the video's own soundtrack drives the effect, mixing would play it twice.
pub fn effective_audio_mode(mode: AudioMode, video: &Path, audio: &Path) -> AudioMode {
    if mode == AudioMode::Mix && video == audio {
        AudioMode::Driving
    } else {
        mode
    }
}
