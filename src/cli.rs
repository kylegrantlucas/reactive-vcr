use crate::video::encoder::AudioMode;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "reactive-vcr",
    version,
    about = "Audio-reactive ntsc-rs VHS and bender-rs circuit-bend renderer"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Render a video with the effect modulated by an audio file.
    Render(RenderArgs),
    /// Dump per-frame audio features and resulting parameter values as CSV.
    Analyze(AnalyzeArgs),
    /// List every ntsc-rs and bender-rs parameter that can be targeted.
    Params {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    #[arg(long)]
    pub video: PathBuf,
    #[arg(long)]
    pub audio: PathBuf,
    #[arg(long)]
    pub map: PathBuf,
    #[arg(short, long)]
    pub output: PathBuf,
    /// Scale factor in (0, 1] for a quick low-resolution preview.
    #[arg(long)]
    pub preview: Option<f64>,
    /// Start offset in seconds, applied to video and audio.
    #[arg(long)]
    pub start: Option<f64>,
    /// Duration in seconds.
    #[arg(long)]
    pub duration: Option<f64>,
    #[arg(long, value_enum, default_value_t = AudioMode::Mix)]
    pub audio_mode: AudioMode,
    /// Extra ffmpeg output arguments, for example "-c:v libx265 -crf 22".
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub ffmpeg_out_args: String,
    #[arg(short, long)]
    pub quiet: bool,
}

#[derive(clap::Args, Debug)]
pub struct AnalyzeArgs {
    #[arg(long)]
    pub audio: PathBuf,
    #[arg(long)]
    pub map: PathBuf,
    #[arg(long)]
    pub fps: f64,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long)]
    pub start: Option<f64>,
    #[arg(long)]
    pub duration: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_output_args_accept_documented_hyphenated_value() {
        let cli = Cli::try_parse_from([
            "reactive-vcr",
            "render",
            "--video",
            "in.mp4",
            "--audio",
            "song.wav",
            "--map",
            "map.toml",
            "--output",
            "out.mp4",
            "--ffmpeg-out-args",
            "-c:v libx265 -crf 22",
        ])
        .unwrap();
        let Command::Render(render) = cli.command else {
            panic!("expected render command");
        };
        assert_eq!(render.ffmpeg_out_args, "-c:v libx265 -crf 22");
    }
}
