use anyhow::Result;
use clap::Parser;
use reactive_vcr::cli::{Cli, Command};
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::render::{RenderOptions, render};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Render(args) => {
            let output = args.output.clone();
            let summary = render(&RenderOptions {
                video: args.video,
                audio: args.audio,
                map: args.map,
                output: output.clone(),
                preview: args.preview,
                start_s: args.start,
                duration_s: args.duration,
                audio_mode: args.audio_mode,
                extra_out_args: args
                    .ffmpeg_out_args
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                quiet: args.quiet,
            })?;
            eprintln!(
                "rendered {} frames at {}x{} @ {:.3} fps → {}",
                summary.frames,
                summary.width,
                summary.height,
                summary.fps,
                output.display()
            );
        }
        Command::Analyze(args) => {
            reactive_vcr::analyze::analyze(&reactive_vcr::analyze::AnalyzeOptions {
                audio: args.audio,
                map: args.map,
                fps: args.fps,
                output: args.output,
                start_s: args.start,
                duration_s: args.duration,
            })?
        }
        Command::Params { json } => print_params(json)?,
    }
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
