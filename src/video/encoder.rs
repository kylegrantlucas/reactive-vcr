use crate::video::process::ManagedChild;
use anyhow::{Context as _, Result, bail};
use std::io::Write;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AudioMode {
    Mix,
    Driving,
    Original,
    None,
}

#[derive(Debug, Clone)]
pub struct EncodeOptions {
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub start_s: Option<f64>,
    pub driving_audio: Option<PathBuf>,
    pub original_video: Option<PathBuf>,
    pub original_has_audio: bool,
    pub audio_mode: AudioMode,
    pub extra_out_args: Vec<String>,
}

pub fn encoder_args(options: &EncodeOptions) -> Vec<String> {
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgb24".into(),
        "-s".into(),
        format!("{}x{}", options.width, options.height),
        "-r".into(),
        options.fps.to_string(),
        "-i".into(),
        "pipe:0".into(),
    ];
    let push_input = |args: &mut Vec<String>, path: &PathBuf| {
        if let Some(start_s) = options.start_s {
            args.extend(["-ss".into(), start_s.to_string()]);
        }
        args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    };
    let use_original = options.original_has_audio && options.original_video.is_some();
    let mode = match (options.audio_mode, &options.driving_audio) {
        (AudioMode::Mix, Some(_)) if use_original => AudioMode::Mix,
        (AudioMode::Mix, Some(_)) => AudioMode::Driving,
        (AudioMode::Mix, None) if use_original => AudioMode::Original,
        (AudioMode::Mix, None) => AudioMode::None,
        (AudioMode::Driving, None) => AudioMode::None,
        (AudioMode::Original, _) if !use_original => AudioMode::None,
        (mode, _) => mode,
    };
    match mode {
        AudioMode::Mix => {
            push_input(
                &mut args,
                options.driving_audio.as_ref().expect("driving input"),
            );
            push_input(
                &mut args,
                options.original_video.as_ref().expect("original input"),
            );
            args.extend([
                "-filter_complex".into(),
                "[1:a][2:a]amix=inputs=2:duration=first:normalize=0,apad[a]".into(),
                "-map".into(),
                "0:v".into(),
                "-map".into(),
                "[a]".into(),
            ]);
        }
        AudioMode::Driving => {
            push_input(
                &mut args,
                options.driving_audio.as_ref().expect("driving input"),
            );
            args.extend([
                "-map".into(),
                "0:v".into(),
                "-map".into(),
                "1:a".into(),
                "-af".into(),
                "apad".into(),
            ]);
        }
        AudioMode::Original => {
            push_input(
                &mut args,
                options.original_video.as_ref().expect("original input"),
            );
            args.extend([
                "-map".into(),
                "0:v".into(),
                "-map".into(),
                "1:a".into(),
                "-af".into(),
                "apad".into(),
            ]);
        }
        AudioMode::None => args.extend(["-map".into(), "0:v".into(), "-an".into()]),
    }
    args.extend([
        "-c:v".into(),
        "libx264".into(),
        "-crf".into(),
        "18".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ]);
    if mode != AudioMode::None {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }
    args.extend(options.extra_out_args.iter().cloned());
    args.extend([
        "-shortest".into(),
        options.output.to_string_lossy().into_owned(),
    ]);
    args
}

fn validate_options(options: &EncodeOptions) -> Result<()> {
    if !options.fps.is_finite() || options.fps <= 0.0 {
        bail!("encoder fps must be finite and positive");
    }
    if options
        .start_s
        .is_some_and(|start_s| !start_s.is_finite() || start_s < 0.0)
    {
        bail!("encoder start time must be finite and nonnegative");
    }
    Ok(())
}

pub struct FrameWriter {
    process: ManagedChild,
    stdin: Option<ChildStdin>,
}

impl FrameWriter {
    pub fn spawn(options: &EncodeOptions) -> Result<Self> {
        validate_options(options)?;
        let mut child = Command::new("ffmpeg")
            .args(encoder_args(options))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning ffmpeg encoder")?;
        let stdin = child.stdin.take().expect("piped stdin");
        let process = ManagedChild::new(child)?;
        Ok(Self {
            process,
            stdin: Some(stdin),
        })
    }

    pub fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        let stdin = self.stdin.as_mut().expect("open ffmpeg encoder stdin");
        if let Err(write_error) = stdin.write_all(frame) {
            self.stdin.take();
            let primary =
                anyhow::Error::new(write_error).context("writing frame to ffmpeg encoder");
            return match self.process.wait_success("ffmpeg encoder") {
                Ok(()) => Err(primary),
                Err(child_error) => Err(primary.context(format!("{child_error:#}"))),
            };
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.stdin.take();
        self.process.wait_success("ffmpeg encoder")
    }
}

impl Drop for FrameWriter {
    fn drop(&mut self) {
        self.stdin.take();
        self.process.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn base() -> EncodeOptions {
        EncodeOptions {
            output: "out.mp4".into(),
            width: 640,
            height: 360,
            fps: 30.0,
            start_s: None,
            driving_audio: Some("song.wav".into()),
            original_video: Some("in.mp4".into()),
            original_has_audio: true,
            audio_mode: AudioMode::Mix,
            extra_out_args: vec![],
        }
    }

    #[test]
    fn mix_mode_uses_amix_and_pads_selected_audio() {
        let a = encoder_args(&base()).join(" ");
        assert!(
            a.contains("-f rawvideo -pix_fmt rgb24 -s 640x360 -r 30 -i pipe:0"),
            "{a}"
        );
        assert!(a.contains("-i song.wav"), "{a}");
        assert!(a.contains("-i in.mp4"), "{a}");
        assert!(
            a.contains("[1:a][2:a]amix=inputs=2:duration=first:normalize=0,apad[a]"),
            "{a}"
        );
        assert!(a.contains("-map 0:v -map [a]"), "{a}");
        assert!(a.ends_with("-shortest out.mp4"), "{a}");
    }

    #[test]
    fn mix_without_original_audio_is_padded_driving_only() {
        let o = EncodeOptions {
            original_has_audio: false,
            ..base()
        };
        let a = encoder_args(&o).join(" ");
        assert!(!a.contains("amix") && a.contains("-af apad"), "{a}");
        assert!(a.contains("-map 0:v -map 1:a"), "{a}");
        assert!(!a.contains("in.mp4"));
    }

    #[test]
    fn other_modes_and_start_offset_and_extra_args() {
        let o = EncodeOptions {
            audio_mode: AudioMode::Original,
            start_s: Some(2.0),
            ..base()
        };
        let a = encoder_args(&o).join(" ");
        assert!(
            a.contains("-ss 2 -i in.mp4")
                && !a.contains("song.wav")
                && a.contains("-af apad")
                && a.contains("-map 0:v -map 1:a"),
            "{a}"
        );
        let o = EncodeOptions {
            audio_mode: AudioMode::None,
            ..base()
        };
        let a = encoder_args(&o).join(" ");
        assert!(a.contains("-an") && !a.contains("-i song.wav"), "{a}");
        let o = EncodeOptions {
            audio_mode: AudioMode::Driving,
            extra_out_args: vec!["-c:v".into(), "libx265".into()],
            ..base()
        };
        let a = encoder_args(&o).join(" ");
        assert!(
            a.contains("-map 0:v -map 1:a")
                && a.contains("-c:v libx264")
                && a.contains("-c:v libx265"),
            "{a}"
        );
        assert!(
            a.find("libx265").unwrap() > a.find("libx264").unwrap(),
            "extra args come after defaults so they override"
        );
    }

    #[test]
    fn spawn_rejects_invalid_timeline_and_fps_before_invoking_ffmpeg() {
        let base = EncodeOptions {
            output: "/definitely/missing/out.mp4".into(),
            ..base()
        };
        for fps in [-1.0, 0.0, f64::NAN, f64::INFINITY] {
            assert!(
                FrameWriter::spawn(&EncodeOptions {
                    fps,
                    ..base.clone()
                })
                .is_err()
            );
        }
        for start_s in [Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            assert!(
                FrameWriter::spawn(&EncodeOptions {
                    start_s,
                    ..base.clone()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn encoder_errors_include_ffmpeg_stderr() {
        if !crate::audio::decode::tool_available("ffmpeg") {
            eprintln!("skip");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let options = EncodeOptions {
            output: dir.path().join("out.mp4"),
            width: 2,
            height: 2,
            fps: 1.0,
            start_s: None,
            driving_audio: None,
            original_video: None,
            original_has_audio: false,
            audio_mode: AudioMode::None,
            extra_out_args: vec!["-definitely-invalid-option".into()],
        };
        let mut writer = FrameWriter::spawn(&options).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let error = loop {
            match writer.write_frame(&[0; 12]) {
                Err(error) => break error,
                Ok(()) if Instant::now() >= deadline => {
                    panic!("ffmpeg did not close stdin after reporting its invalid option")
                }
                Ok(()) => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        assert!(
            format!("{error:#}").contains("definitely-invalid-option"),
            "{error:#}"
        );
    }

    fn make_clip(path: &std::path::Path, duration: f64, audio_duration: f64) {
        let video = format!("testsrc2=size=64x48:rate=10:duration={duration}");
        let audio = format!("sine=frequency=440:duration={audio_duration}");
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &video,
                    "-f",
                    "lavfi",
                    "-i",
                    &audio,
                    "-shortest",
                    "-pix_fmt",
                    "yuv420p"
                ])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    fn make_video_with_audio(path: &std::path::Path, video_duration: f64, audio_duration: f64) {
        let video = format!("testsrc2=size=64x48:rate=10:duration={video_duration}");
        let audio = format!("sine=frequency=440:duration={audio_duration}");
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v", "error", "-y", "-f", "lavfi", "-i", &video, "-f", "lavfi", "-i", &audio,
                    "-map", "0:v", "-map", "1:a", "-pix_fmt", "yuv420p",
                ])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    fn output_frame_count(path: &std::path::Path) -> usize {
        let output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=nb_read_frames",
                "-of",
                "default=nokey=1:noprint_wrappers=1",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn output_container_duration(path: &std::path::Path) -> f64 {
        let output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nokey=1:noprint_wrappers=1",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn render_frames(options: &EncodeOptions, frames: usize) {
        let mut writer = FrameWriter::spawn(options).unwrap();
        let frame = vec![0u8; options.width as usize * options.height as usize * 3];
        for _ in 0..frames {
            writer.write_frame(&frame).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn selected_audio_is_padded_to_full_video_duration() {
        if !crate::audio::decode::tool_available("ffmpeg")
            || !crate::audio::decode::tool_available("ffprobe")
        {
            eprintln!("skip");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let driving = dir.path().join("driving.wav");
        let original = dir.path().join("original.mp4");
        make_clip(&driving, 0.5, 0.5);
        make_video_with_audio(&original, 2.0, 0.5);
        for mode in [AudioMode::Driving, AudioMode::Original, AudioMode::Mix] {
            let output = dir.path().join(format!("{mode:?}.mp4"));
            render_frames(
                &EncodeOptions {
                    output: output.clone(),
                    width: 64,
                    height: 48,
                    fps: 10.0,
                    start_s: None,
                    driving_audio: Some(driving.clone()),
                    original_video: Some(original.clone()),
                    original_has_audio: true,
                    audio_mode: mode,
                    extra_out_args: vec![],
                },
                20,
            );
            assert_eq!(output_frame_count(&output), 20, "{mode:?}");
            let info = crate::probe::probe(&output).unwrap();
            assert!(info.has_audio, "{mode:?}");
        }
    }

    #[test]
    fn shortest_caps_audio_that_outlasts_video() {
        if !crate::audio::decode::tool_available("ffmpeg")
            || !crate::audio::decode::tool_available("ffprobe")
        {
            eprintln!("skip");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let driving = dir.path().join("driving.wav");
        make_clip(&driving, 3.0, 3.0);
        let output = dir.path().join("out.mp4");
        render_frames(
            &EncodeOptions {
                output: output.clone(),
                width: 64,
                height: 48,
                fps: 10.0,
                start_s: None,
                driving_audio: Some(driving),
                original_video: None,
                original_has_audio: false,
                audio_mode: AudioMode::Driving,
                extra_out_args: vec![],
            },
            20,
        );
        assert_eq!(output_frame_count(&output), 20);
        let duration = output_container_duration(&output);
        assert!(
            (duration - 2.0).abs() < 0.15,
            "long audio was not capped at the 2 second video duration: {duration}"
        );
    }
}
