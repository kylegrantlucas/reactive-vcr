use anyhow::{Context as _, Result, bail};
use std::path::Path;
use std::process::{Command, Stdio};

pub const SAMPLE_RATE: u32 = 48_000;

/// True if the named tool can execute and report its version.
pub fn tool_available(name: &str) -> bool {
    Command::new(name)
        .args(["-v", "error", "-version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Decode any audio file to mono f32 at SAMPLE_RATE using ffmpeg.
pub fn decode_mono_f32(path: &Path) -> Result<Vec<f32>> {
    if !path.exists() {
        bail!("audio file not found: {}", path.display());
    }

    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .arg("-")
        .stdin(Stdio::null())
        .output()
        .context("running ffmpeg to decode audio (is ffmpeg on PATH?)")?;
    if !output.status.success() {
        bail!(
            "ffmpeg failed decoding {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let samples = output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect::<Vec<f32>>();
    if samples.is_empty() {
        bail!("ffmpeg produced no audio samples from {}", path.display());
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn decodes_one_second_sine_to_48k_mono() {
        if !tool_available("ffmpeg") {
            eprintln!("skipping: ffmpeg not found");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("sine.wav");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-ac",
                "2",
                "-ar",
                "44100",
            ])
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success());
        let samples = decode_mono_f32(&wav).unwrap();
        assert!(
            (samples.len() as i64 - 48_000).abs() < 100,
            "got {} samples",
            samples.len()
        );
        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
        assert!(rms > 0.05 && rms <= 1.0, "rms {rms}");
    }

    #[test]
    fn missing_file_is_an_error() {
        if !tool_available("ffmpeg") {
            return;
        }
        assert!(decode_mono_f32(Path::new("/nonexistent/x.wav")).is_err());
    }
}
