use clap::{Parser, Subcommand, ValueEnum};
use reactive_vcr_gst::AudioMode;
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
    /// Play a video in real time with the effect modulated by an audio file.
    Play(PlayArgs),
    /// Process live video in real time with the effect driven by live audio.
    Stream(StreamArgs),
    /// Dump per-frame audio features and resulting parameter values as CSV.
    Analyze(AnalyzeArgs),
    /// List every ntsc-rs and bender-rs parameter that can be targeted.
    Params {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AudioModeArg {
    Mix,
    Driving,
    Original,
    None,
}

impl From<AudioModeArg> for AudioMode {
    fn from(mode: AudioModeArg) -> Self {
        match mode {
            AudioModeArg::Mix => AudioMode::Mix,
            AudioModeArg::Driving => AudioMode::Driving,
            AudioModeArg::Original => AudioMode::Original,
            AudioModeArg::None => AudioMode::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CodecArg {
    /// H.264 and AAC in MP4.
    H264,
    /// Lossless FFV1 and FLAC in Matroska.
    Ffv1,
}

#[derive(clap::Args, Debug)]
pub struct Timeline {
    #[arg(long)]
    pub video: PathBuf,
    #[arg(long)]
    pub audio: PathBuf,
    #[arg(long)]
    pub map: PathBuf,
    /// Scale factor in (0, 1] for faster, lower-resolution output.
    #[arg(long)]
    pub preview: Option<f64>,
    /// Start offset in seconds, applied to video and audio.
    #[arg(long)]
    pub start: Option<f64>,
    /// Duration in seconds.
    #[arg(long)]
    pub duration: Option<f64>,
    #[arg(long, value_enum, default_value_t = AudioModeArg::Mix)]
    pub audio_mode: AudioModeArg,
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    #[command(flatten)]
    pub timeline: Timeline,
    /// Output file; .mp4 implies H.264, .mkv implies FFV1.
    #[arg(short, long)]
    pub output: PathBuf,
    /// Override the codec implied by the output extension.
    #[arg(long, value_enum)]
    pub codec: Option<CodecArg>,
    /// H.264 quantizer; lower is higher quality and 0 is lossless.
    #[arg(long, default_value_t = 18, value_parser = clap::value_parser!(u32).range(0..=51))]
    pub quantizer: u32,
    #[arg(short, long)]
    pub quiet: bool,
}

#[derive(clap::Args, Debug)]
pub struct PlayArgs {
    /// Edits to the mapping file and its presets apply while playing.
    #[command(flatten)]
    pub timeline: Timeline,
    /// Restart at the end instead of exiting.
    #[arg(long = "loop")]
    pub looping: bool,
    /// GStreamer video sink, as a gst-launch description.
    #[arg(long, default_value = "autovideosink")]
    pub video_sink: String,
    /// GStreamer audio sink, as a gst-launch description.
    #[arg(long, default_value = "autoaudiosink")]
    pub audio_sink: String,
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

#[derive(clap::Args, Debug)]
pub struct StreamArgs {
    /// Mapping file; edits to it and its presets apply while streaming.
    #[arg(long)]
    pub map: PathBuf,
    /// GStreamer video source, as a gst-launch description.
    #[arg(long, default_value = "autovideosrc")]
    pub video_source: String,
    /// GStreamer audio source that drives the effect, as a gst-launch description.
    #[arg(long, default_value = "autoaudiosrc")]
    pub audio_source: String,
    /// Processing height in lines; width follows the source's aspect ratio.
    #[arg(long, default_value_t = 480, value_parser = clap::value_parser!(u32).range(2..=2160))]
    pub lines: u32,
    /// GStreamer video sink, as a gst-launch description.
    #[arg(long, default_value = "autovideosink")]
    pub video_sink: String,
    /// GStreamer audio sink; the default discards audio so a microphone can't feed back.
    #[arg(long, default_value = "fakesink")]
    pub audio_sink: String,
    #[arg(short, long)]
    pub quiet: bool,
}
