use crate::video::process::ManagedChild;
use anyhow::{Context as _, Result, bail};
use std::io::Read;
use std::path::PathBuf;
use std::process::{ChildStdout, Command, Stdio};

#[derive(Debug, Clone)]
pub struct DecodeOptions {
    pub input: PathBuf,
    /// Constant frame rate shared with feature extraction and the encoder.
    pub fps: f64,
    pub start_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub width: u32,
    pub height: u32,
}

pub fn even_scaled(dim: u32, scale: f64) -> u32 {
    let scaled = ((dim as f64 * scale) / 2.0).floor() as u32 * 2;
    scaled.max(2)
}

pub fn decoder_args(options: &DecodeOptions) -> Vec<String> {
    let mut args = vec!["-v".into(), "error".into()];
    if let Some(start_s) = options.start_s {
        args.extend(["-ss".into(), start_s.to_string()]);
    }
    if let Some(duration_s) = options.duration_s {
        args.extend(["-t".into(), duration_s.to_string()]);
    }
    args.extend(["-i".into(), options.input.to_string_lossy().into_owned()]);
    args.extend(["-map".into(), "0:v:0".into()]);
    args.extend([
        "-vf".into(),
        format!(
            "fps={},scale={}:{}",
            options.fps, options.width, options.height
        ),
    ]);
    args.extend([
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgb24".into(),
        "-".into(),
    ]);
    args
}

fn validate_options(options: &DecodeOptions) -> Result<usize> {
    if !options.fps.is_finite() || options.fps <= 0.0 {
        bail!("decoder fps must be finite and positive");
    }
    if options
        .start_s
        .is_some_and(|start_s| !start_s.is_finite() || start_s < 0.0)
    {
        bail!("decoder start time must be finite and nonnegative");
    }
    if options
        .duration_s
        .is_some_and(|duration_s| !duration_s.is_finite() || duration_s <= 0.0)
    {
        bail!("decoder duration must be finite and positive");
    }
    (options.width as usize)
        .checked_mul(options.height as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| anyhow::anyhow!("decoder frame size overflows usize"))
}

pub struct FrameReader {
    process: ManagedChild,
    stdout: Option<ChildStdout>,
    frame_bytes: usize,
}

impl FrameReader {
    pub fn spawn(options: &DecodeOptions) -> Result<Self> {
        let frame_bytes = validate_options(options)?;
        let mut child = Command::new("ffmpeg")
            .args(decoder_args(options))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning ffmpeg decoder")?;
        let stdout = child.stdout.take().expect("piped stdout");
        let process = ManagedChild::new(child)?;
        Ok(Self {
            process,
            stdout: Some(stdout),
            frame_bytes,
        })
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    pub fn read_frame(&mut self, buffer: &mut [u8]) -> Result<bool> {
        assert_eq!(buffer.len(), self.frame_bytes);
        let stdout = self.stdout.as_mut().expect("open ffmpeg decoder stdout");
        let mut filled = 0;
        while filled < buffer.len() {
            let read = stdout
                .read(&mut buffer[filled..])
                .context("reading frame from ffmpeg")?;
            if read == 0 {
                if filled == 0 {
                    return Ok(false);
                }
                bail!(
                    "ffmpeg decoder ended mid-frame ({filled} of {} bytes)",
                    buffer.len()
                );
            }
            filled += read;
        }
        Ok(true)
    }

    pub fn finish(mut self) -> Result<()> {
        self.stdout.take();
        self.process.wait_success("ffmpeg decoder")
    }
}

impl Drop for FrameReader {
    fn drop(&mut self) {
        self.stdout.take();
        self.process.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_scaling() {
        assert_eq!(even_scaled(1280, 0.5), 640);
        assert_eq!(even_scaled(1281, 1.0), 1280);
        assert_eq!(even_scaled(639, 0.5), 318);
    }

    #[test]
    fn args_include_trim_and_scale() {
        let o = DecodeOptions {
            input: "in.mp4".into(),
            fps: 30.0,
            start_s: Some(1.5),
            duration_s: Some(2.0),
            width: 640,
            height: 360,
        };
        let a = decoder_args(&o).join(" ");
        assert!(a.starts_with("-v error -ss 1.5 -t 2 -i in.mp4"), "{a}");
        assert!(a.contains("-map 0:v:0"), "{a}");
        assert!(a.contains("-vf fps=30,scale=640:360"), "{a}");
        assert!(a.ends_with("-f rawvideo -pix_fmt rgb24 -"), "{a}");
        let o2 = DecodeOptions {
            start_s: None,
            duration_s: None,
            ..o
        };
        assert!(!decoder_args(&o2).contains(&"-ss".to_string()));
    }

    #[test]
    fn spawn_rejects_invalid_timeline_before_invoking_ffmpeg() {
        let base = DecodeOptions {
            input: "/definitely/missing.mp4".into(),
            fps: 30.0,
            start_s: None,
            duration_s: None,
            width: 2,
            height: 2,
        };
        for fps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let error = FrameReader::spawn(&DecodeOptions {
                fps,
                ..base.clone()
            })
            .err()
            .expect("invalid fps must be rejected");
            assert!(format!("{error:#}").contains("decoder fps"));
        }
        for start_s in [Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            assert!(
                FrameReader::spawn(&DecodeOptions {
                    start_s,
                    ..base.clone()
                })
                .is_err()
            );
        }
        for duration_s in [Some(-1.0), Some(0.0), Some(f64::NAN), Some(f64::INFINITY)] {
            assert!(
                FrameReader::spawn(&DecodeOptions {
                    duration_s,
                    ..base.clone()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn reads_frames_from_synthetic_clip() {
        if !crate::audio::decode::tool_available("ffmpeg") {
            eprintln!("skip");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("c.mp4");
        assert!(
            std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=64x48:rate=10:duration=1",
                    "-pix_fmt",
                    "yuv420p"
                ])
                .arg(&clip)
                .status()
                .unwrap()
                .success()
        );
        let o = DecodeOptions {
            input: clip,
            fps: 10.0,
            start_s: None,
            duration_s: None,
            width: 32,
            height: 24,
        };
        let mut r = FrameReader::spawn(&o).unwrap();
        let mut buf = vec![0u8; r.frame_bytes()];
        let mut n = 0;
        while r.read_frame(&mut buf).unwrap() {
            n += 1;
        }
        r.finish().unwrap();
        assert_eq!(n, 10);
    }

    #[test]
    fn decodes_the_first_video_stream_selected_by_probe() {
        if !crate::audio::decode::tool_available("ffmpeg") {
            eprintln!("skip");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("two-streams.mp4");
        assert!(
            std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=red:size=16x16:rate=5:duration=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=blue:size=32x32:rate=10:duration=1",
                    "-map",
                    "0:v",
                    "-map",
                    "1:v",
                    "-disposition:v:0",
                    "0",
                    "-disposition:v:1",
                    "default",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(&clip)
                .status()
                .unwrap()
                .success()
        );
        let mut reader = FrameReader::spawn(&DecodeOptions {
            input: clip,
            fps: 5.0,
            start_s: None,
            duration_s: None,
            width: 16,
            height: 16,
        })
        .unwrap();
        let mut frame = vec![0; reader.frame_bytes()];
        let mut count = 0;
        while reader.read_frame(&mut frame).unwrap() {
            count += 1;
        }
        reader.finish().unwrap();
        assert_eq!(count, 5);
    }
}
