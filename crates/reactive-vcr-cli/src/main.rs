mod cli;

use anyhow::{Result, bail};
use clap::Parser;
use cli::{Cli, CodecArg, Command, RenderArgs};
use reactive_vcr::mapping;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::{select_timeline, selection_features, validate_output_is_distinct};
use reactive_vcr_gst::analyze::{AnalyzeOptions, analyze};
use reactive_vcr_gst::decode::decode_mono_f32;
use reactive_vcr_gst::play::{PlayOptions, play};
use reactive_vcr_gst::probe::probe;
use reactive_vcr_gst::render::{OutputCodec, RenderJob, RenderSettings, RenderStatus};
use reactive_vcr_gst::{AudioMode, Program};
use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

fn main() -> Result<()> {
    let cli = Cli::parse();
    reactive_vcr_gst::main_thread(move || run(cli))
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Render(args) => render(args),
        Command::Play(args) => {
            let quiet = args.quiet;
            let timeline = args.timeline;
            let summary = play(&PlayOptions {
                video: timeline.video,
                audio: timeline.audio,
                map: timeline.map,
                preview: timeline.preview,
                start_s: timeline.start,
                duration_s: timeline.duration,
                audio_mode: timeline.audio_mode.into(),
                looping: args.looping,
                video_sink: args.video_sink,
                audio_sink: args.audio_sink,
                quiet,
            })?;
            if !quiet {
                eprintln!("played {} frames", summary.frames);
            }
            Ok(())
        }
        Command::Analyze(args) => {
            let output = args.output.clone();
            let frames = analyze(&AnalyzeOptions {
                audio: args.audio,
                map: args.map,
                fps: args.fps,
                output: args.output,
                start_s: args.start,
                duration_s: args.duration,
            })?;
            eprintln!("wrote {frames} frames of features to {}", output.display());
            Ok(())
        }
        Command::Params { json } => print_params(json),
    }
}

fn render(args: RenderArgs) -> Result<()> {
    let timeline = args.timeline;
    let codec = match args.codec {
        Some(CodecArg::H264) => OutputCodec::H264 {
            quantizer: args.quantizer,
        },
        Some(CodecArg::Ffv1) => OutputCodec::Ffv1,
        None => match OutputCodec::from_extension(&args.output) {
            Some(OutputCodec::H264 { .. }) => OutputCodec::H264 {
                quantizer: args.quantizer,
            },
            Some(codec) => codec,
            None => bail!(
                "cannot infer a codec from {}; use .mp4 or .mkv, or pass --codec",
                args.output.display()
            ),
        },
    };

    let info = probe(&timeline.video)?;
    let selection = select_timeline(&info, timeline.preview, timeline.start, timeline.duration)?;
    let catalog = Arc::new(ParamCatalog::new());
    let map = mapping::load(&timeline.map, &catalog)?;
    validate_output_is_distinct(&args.output, &[("mapping", &timeline.map)])?;
    validate_output_is_distinct(&args.output, &map.preset_inputs())?;
    let samples = decode_mono_f32(&timeline.audio)?;
    let features = selection_features(&samples, &map, &selection)?;
    let program = Arc::new(Program::new(catalog, &map, &features, &selection));

    let job = RenderJob::start(RenderSettings {
        video: timeline.video,
        audio: timeline.audio,
        output: args.output.clone(),
        codec,
        audio_mode: AudioMode::from(timeline.audio_mode),
        selection,
        has_original_audio: info.has_audio,
        program,
    })?;
    let mut stderr = std::io::stderr();
    loop {
        match job.status() {
            RenderStatus::Running { progress } => {
                if !args.quiet {
                    write!(stderr, "\rrendering {:5.1}%", progress * 100.0)?;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            RenderStatus::Done => break,
            RenderStatus::Failed(message) => {
                if !args.quiet {
                    writeln!(stderr)?;
                }
                bail!("render failed: {message}");
            }
            RenderStatus::Cancelled => bail!("render cancelled"),
        }
    }
    if !args.quiet {
        writeln!(stderr, "\r")?;
    }
    eprintln!(
        "rendered {} frames at {}x{} @ {:.3} fps → {}",
        job.frames_rendered(),
        selection.width,
        selection.height,
        selection.fps(),
        args.output.display()
    );
    Ok(())
}

fn print_params(json: bool) -> Result<()> {
    let catalog = ParamCatalog::new();
    if json {
        let items: Vec<serde_json::Value> = catalog
            .iter()
            .map(|parameter| {
                let (min, max) = parameter.kind.bounds();
                serde_json::json!({
                    "id": parameter.name,
                    "label": parameter.label,
                    "kind": parameter.kind.describe(),
                    "min": min,
                    "max": max,
                    "group": parameter.is_group,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        for parameter in catalog.iter() {
            println!(
                "{:<40} {:<32} {}",
                parameter.name,
                parameter.label,
                parameter.kind.describe()
            );
        }
    }
    Ok(())
}
