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
    /// Compile a mapping as `stream` and `render` would, without opening any media.
    ///
    /// Exits 0 when the mapping is valid, 1 when it is invalid, and 2 when it can't be read
    /// or the arguments are wrong.
    Check(CheckArgs),
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
    /// GStreamer video source, as a gst-launch description: a camera, or a file or pipe,
    /// which plays every frame in real time.
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
    /// Treat the driving audio as silence while its level is below this many dBFS, such as
    /// `-60`; set it a few dB above the input's noise floor. Opens above the level, closes
    /// 3 dB below it.
    #[arg(long, value_name = "DBFS", allow_negative_numbers = true, value_parser = gate_db)]
    pub gate_db: Option<f32>,
    /// Send UDP JSON telemetry to HOST:PORT: a start message, frame state, and fatal errors.
    #[arg(long, value_name = "HOST:PORT", requires = "telemetry_id")]
    pub telemetry: Option<String>,
    /// Identifies this stream in every telemetry message.
    #[arg(long, value_name = "ID", requires = "telemetry")]
    pub telemetry_id: Option<String>,
    /// Most telemetry frame messages per second.
    #[arg(long, value_name = "N", default_value_t = 15, requires = "telemetry",
          value_parser = clap::value_parser!(u32).range(1..=1000))]
    pub telemetry_hz: u32,
    #[arg(short, long)]
    pub quiet: bool,
}

#[derive(clap::Args, Debug)]
pub struct CheckArgs {
    #[arg(long)]
    pub map: PathBuf,
    /// Print one JSON line: `{"ok":true}` or `{"ok":false,"errors":[...]}`.
    #[arg(long)]
    pub json: bool,
}

fn gate_db(value: &str) -> Result<f32, String> {
    let db: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if (-120.0..=0.0).contains(&db) {
        Ok(db)
    } else {
        Err(format!("{db} dBFS is outside -120..=0"))
    }
}
