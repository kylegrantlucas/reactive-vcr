use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_s: f64,
    pub has_audio: bool,
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    #[serde(default)]
    format: Format,
}

#[derive(Deserialize, Default)]
struct Format {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: String,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
}

pub fn parse_rate(s: &str) -> Result<f64> {
    let value = match s.split_once('/') {
        Some((numerator, denominator)) => {
            let numerator: f64 = numerator.trim().parse().context("frame rate numerator")?;
            let denominator: f64 = denominator
                .trim()
                .parse()
                .context("frame rate denominator")?;
            if denominator == 0.0 {
                bail!("frame rate has zero denominator: {s}");
            }
            numerator / denominator
        }
        None => s.trim().parse().context("frame rate")?,
    };
    if !value.is_finite() || value <= 0.0 {
        bail!("invalid frame rate {s:?}");
    }
    Ok(value)
}

pub fn parse_probe_json(json: &str) -> Result<VideoInfo> {
    let probe: Probe = serde_json::from_str(json).context("parsing ffprobe JSON")?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type == "video")
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let rate = video
        .avg_frame_rate
        .as_deref()
        .filter(|rate| !rate.starts_with("0/"))
        .or(video.r_frame_rate.as_deref())
        .ok_or_else(|| anyhow!("video stream has no frame rate"))?;
    let duration_s: f64 = video
        .duration
        .as_deref()
        .or(probe.format.duration.as_deref())
        .ok_or_else(|| anyhow!("cannot determine video duration"))?
        .parse()
        .context("duration")?;
    if !duration_s.is_finite() || duration_s < 0.0 {
        bail!("invalid video duration {duration_s}");
    }

    Ok(VideoInfo {
        width: video
            .width
            .ok_or_else(|| anyhow!("video stream has no width"))?,
        height: video
            .height
            .ok_or_else(|| anyhow!("video stream has no height"))?,
        fps: parse_rate(rate)?,
        duration_s,
        has_audio: probe
            .streams
            .iter()
            .any(|stream| stream.codec_type == "audio"),
    })
}

pub fn probe(path: &Path) -> Result<VideoInfo> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("running ffprobe (is it on PATH?)")?;
    if !output.status.success() {
        bail!(
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_probe_json(&String::from_utf8_lossy(&output.stdout))
        .with_context(|| format!("probing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"streams":[
      {"codec_type":"video","width":1280,"height":720,"r_frame_rate":"30000/1001","avg_frame_rate":"30000/1001","duration":"4.004000"},
      {"codec_type":"audio","sample_rate":"48000"}],
      "format":{"duration":"4.021000"}}"#;

    #[test]
    fn parses_ffprobe_json() {
        let v = parse_probe_json(JSON).unwrap();
        assert_eq!((v.width, v.height), (1280, 720));
        assert!((v.fps - 29.97).abs() < 0.01);
        assert!((v.duration_s - 4.004).abs() < 1e-6);
        assert!(v.has_audio);
    }

    #[test]
    fn no_audio_and_missing_video_stream() {
        let v = parse_probe_json(r#"{"streams":[{"codec_type":"video","width":2,"height":2,"r_frame_rate":"25/1"}],"format":{"duration":"1.0"}}"#).unwrap();
        assert!(!v.has_audio);
        assert_eq!(v.fps, 25.0);
        assert_eq!(v.duration_s, 1.0);
        assert!(parse_probe_json(r#"{"streams":[],"format":{}}"#).is_err());
    }

    #[test]
    fn parse_rate_forms_and_rejects_nonfinite_values() {
        assert_eq!(parse_rate("25/1").unwrap(), 25.0);
        assert_eq!(parse_rate("30").unwrap(), 30.0);
        assert!(parse_rate("0/0").is_err());
        assert!(parse_rate("inf").is_err());
        assert!(parse_probe_json(r#"{"streams":[{"codec_type":"video","width":2,"height":2,"r_frame_rate":"25/1","duration":"NaN"}],"format":{}}"#).is_err());
    }
}
