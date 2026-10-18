//! Pipeline building blocks shared by playback, preview, rendering, and audio decoding.

use anyhow::{Context as _, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;
use reactive_vcr::timeline::Selection;
use std::path::Path;

/// Which audio accompanies the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioMode {
    /// The driving audio summed with the video's own audio.
    #[default]
    Mix,
    /// Only the audio that drives the effect.
    Driving,
    /// Only the video's own audio.
    Original,
    /// Silence.
    None,
}

impl AudioMode {
    /// The mode that can actually play given which audio sources exist.
    pub fn resolve(self, has_driving: bool, has_original: bool) -> Self {
        match (self, has_driving) {
            (AudioMode::Mix, true) if has_original => AudioMode::Mix,
            (AudioMode::Mix, true) => AudioMode::Driving,
            (AudioMode::Mix, false) if has_original => AudioMode::Original,
            (AudioMode::Mix, false) => AudioMode::None,
            (AudioMode::Driving, false) => AudioMode::None,
            (AudioMode::Original, _) if !has_original => AudioMode::None,
            (mode, _) => mode,
        }
    }
}

/// The plugin set that provides `factory`, for error messages.
fn package_for(factory: &str) -> &'static str {
    match factory {
        "x264enc" => "gst-plugins-ugly",
        "avenc_aac" | "avenc_ffv1" => "gst-libav",
        "mp4mux" | "matroskamux" | "flacenc" | "autoaudiosink" | "autovideosink" => {
            "gst-plugins-good"
        }
        _ => "gst-plugins-base",
    }
}

pub fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory).build().with_context(|| {
        format!(
            "GStreamer element `{factory}` is unavailable; install {}",
            package_for(factory)
        )
    })
}

/// A sink built from a gst-launch description, such as `autovideosink` or
/// `volume volume=0.5 ! autoaudiosink`.
pub fn sink(description: &str) -> Result<gst::Element> {
    let bin = gst::parse::bin_from_description(description, true)
        .with_context(|| format!("invalid GStreamer sink `{description}`"))?;
    Ok(bin.upcast())
}

pub fn seconds(value: f64) -> gst::ClockTime {
    gst::ClockTime::from_nseconds((value * 1e9).round().max(0.0) as u64)
}

/// The selection's start and end as seek positions.
///
/// The start sits a quarter frame late so a preceding frame whose rounded timestamp
/// overlaps it by a hair is dropped; decoders would otherwise clip that frame to the start
/// and show it instead of the one that begins there. The end sits half a frame past the
/// last selected frame, because `videorate` versions round a partial final slot
/// differently; `render` and `play` drop any frame beyond the selection.
pub fn seek_range(selection: &Selection) -> (gst::ClockTime, gst::ClockTime) {
    let fps = selection.fps();
    (
        seconds(selection.start_s + 0.25 / fps),
        seconds(selection.start_s + (selection.frames as f64 + 0.5) / fps),
    )
}

/// `"{error} ({debug})"`, prefixed with the element that posted it.
pub fn error_text(error: &gst::message::Error) -> String {
    let source = error
        .src()
        .map(|source| format!("{}: ", source.name()))
        .unwrap_or_default();
    match error.debug() {
        Some(debug) => format!("{source}{} ({debug})", error.error()),
        None => format!("{source}{}", error.error()),
    }
}

/// Link a decoded pad to `target` if it is still free; otherwise discard its stream.
/// `sync` paces the discard sink with the clock so it neither runs ahead nor skews positions.
fn route(
    pipeline: &gst::Pipeline,
    pad: &gst::Pad,
    target: Option<&gst::Pad>,
    sync: bool,
) -> Result<()> {
    if let Some(target) = target.filter(|target| !target.is_linked()) {
        pad.link(target)?;
        return Ok(());
    }
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", sync)
        .property("async", false)
        .build()
        .context("GStreamer element `fakesink` is unavailable")?;
    pipeline.add(&sink)?;
    sink.sync_state_with_parent()?;
    pad.link(&sink.static_pad("sink").expect("fakesink has a sink pad"))?;
    Ok(())
}

fn kind_of(caps: &gst::CapsRef) -> &str {
    caps.structure(0)
        .map(|s| s.name().as_str())
        .and_then(|name| name.split('/').next())
        .unwrap_or("")
}

/// Whether some demuxer accepts `caps`, so they describe a container, not a single stream.
fn is_container(caps: &gst::Caps) -> bool {
    gst::ElementFactory::factories_with_type(gst::ElementFactoryType::DEMUXER, gst::Rank::MARGINAL)
        .iter()
        .any(|factory| factory.can_sink_any_caps(caps))
}

/// Decode `path`, routing its first video and first audio stream to the given pads, and
/// return the decodebin. Streams without a target stay compressed and are discarded.
pub fn add_source(
    pipeline: &gst::Pipeline,
    path: &Path,
    video: Option<gst::Pad>,
    audio: Option<gst::Pad>,
    sync: bool,
) -> Result<gst::Element> {
    let source = gst::ElementFactory::make("filesrc")
        .property("location", path.to_string_lossy().as_ref())
        .build()
        .context("GStreamer element `filesrc` is unavailable")?;
    let decode = make("decodebin")?;
    pipeline.add_many([&source, &decode])?;
    source.link(&decode)?;
    // gst-libav 1.24's frame-threaded FFV1 decoder sometimes loses a file's last frame at end
    // of stream. Slice threads decode one frame at a time, so they keep every frame and
    // still use every core on sliced FFV1, such as reactive-vcr's own renders.
    decode
        .downcast_ref::<gst::Bin>()
        .expect("decodebin is a bin")
        .connect_deep_element_added(|_, _, element| {
            if element
                .factory()
                .is_some_and(|factory| factory.name() == "avdec_ffv1")
            {
                element.set_property_from_str("thread-type", "slice");
                element.set_property("max-threads", 0i32);
            }
        });

    let (want_video, want_audio) = (video.is_some(), audio.is_some());
    decode.connect("autoplug-continue", false, move |values| {
        // Leave an unwanted elementary stream compressed; containers keep autoplugging.
        let caps = values[2]
            .get::<gst::Caps>()
            .expect("autoplug-continue caps");
        let wanted = match kind_of(&caps) {
            "video" => want_video,
            "audio" => want_audio,
            _ => true,
        };
        Some((wanted || is_container(&caps)).to_value())
    });

    let weak = pipeline.downgrade();
    let label = path.display().to_string();
    let (video_wanted, audio_wanted) = (video.clone(), audio.clone());
    decode.connect_pad_added(move |decode, pad| {
        let Some(pipeline) = weak.upgrade() else {
            return;
        };
        let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
        let kind = kind_of(&caps).to_owned();
        let target = match kind.as_str() {
            "video" => video_wanted.as_ref(),
            "audio" => audio_wanted.as_ref(),
            _ => None,
        };
        if let Err(error) = route(&pipeline, pad, target, sync) {
            gst::element_error!(
                decode,
                gst::CoreError::Negotiation,
                ("could not route {kind} stream: {error:#}")
            );
        }
    });
    decode.connect_no_more_pads(move |decode| {
        for (kind, target) in [("video", &video), ("audio", &audio)] {
            if target.as_ref().is_some_and(|pad| !pad.is_linked()) {
                gst::element_error!(
                    decode,
                    gst::StreamError::WrongType,
                    ("{label} has no {kind} stream")
                );
            }
        }
    });
    Ok(decode)
}

/// `queue ! videoconvert ! videoscale [! videorate] ! capsfilter(RGB, size[, rate])`.
/// The source's pixel aspect ratio carries through, adjusted for any scaling.
/// Returns the chain's input pad and last element.
pub fn video_chain(
    pipeline: &gst::Pipeline,
    selection: &Selection,
    constant_rate: bool,
) -> Result<(gst::Pad, gst::Element)> {
    let mut caps = gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgb)
        .width(selection.width as i32)
        .height(selection.height as i32);
    // Conversion and scaling run on the effect's thread; spread them over every core.
    let mut elements = vec![make("queue")?];
    for factory in ["videoconvert", "videoscale"] {
        let element = make(factory)?;
        element.set_property("n-threads", 0u32);
        elements.push(element);
    }
    if constant_rate {
        elements.push(make("videorate")?);
        caps = caps.framerate(gst::Fraction::new(selection.fps_n, selection.fps_d));
    }
    elements.push(
        gst::ElementFactory::make("capsfilter")
            .property("caps", caps.build())
            .build()
            .context("GStreamer element `capsfilter` is unavailable")?,
    );
    pipeline.add_many(&elements)?;
    gst::Element::link_many(&elements)?;
    let input = elements[0]
        .static_pad("sink")
        .expect("queue has a sink pad");
    Ok((input, elements.pop().expect("chain is not empty")))
}

/// `queue ! audioconvert ! audioresample ! capsfilter` feeding `target`; returns the branch
/// input. Every branch delivers the same format, so a mixer never renegotiates when one
/// input's caps arrive after another's data.
fn audio_branch(pipeline: &gst::Pipeline, target: &gst::Pad) -> Result<gst::Pad> {
    let format = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("rate", 48_000)
        .field("channels", 2)
        .build();
    let elements = [
        make("queue")?,
        make("audioconvert")?,
        make("audioresample")?,
        gst::ElementFactory::make("capsfilter")
            .property("caps", format)
            .build()
            .context("GStreamer element `capsfilter` is unavailable")?,
    ];
    pipeline.add_many(&elements)?;
    gst::Element::link_many(&elements)?;
    elements[3]
        .static_pad("src")
        .expect("capsfilter has a src pad")
        .link(target)?;
    Ok(elements[0]
        .static_pad("sink")
        .expect("queue has a sink pad"))
}

/// Audio inputs for the driving and original tracks, feeding `output`.
///
/// Both tracks are summed without normalization, like render's former
/// `amix=normalize=0`. With `pad_with_silence`, a silent track spans the selection so the
/// output audio always lasts as long as the video.
pub fn audio_inputs(
    pipeline: &gst::Pipeline,
    mode: AudioMode,
    output: &gst::Pad,
    pad_with_silence: bool,
) -> Result<(Option<gst::Pad>, Option<gst::Pad>)> {
    if mode == AudioMode::None {
        return Ok((None, None));
    }
    let direct = mode != AudioMode::Mix && !pad_with_silence;
    let mixer = if direct {
        None
    } else {
        let mixer = make("audiomixer")?;
        pipeline.add(&mixer)?;
        mixer
            .static_pad("src")
            .expect("audiomixer src")
            .link(output)?;
        Some(mixer)
    };
    let input = || -> Result<gst::Pad> {
        match &mixer {
            Some(mixer) => {
                let pad = mixer
                    .request_pad_simple("sink_%u")
                    .context("audiomixer refused a sink pad")?;
                audio_branch(pipeline, &pad)
            }
            None => audio_branch(pipeline, output),
        }
    };
    let driving = matches!(mode, AudioMode::Mix | AudioMode::Driving)
        .then(input)
        .transpose()?;
    let original = matches!(mode, AudioMode::Mix | AudioMode::Original)
        .then(input)
        .transpose()?;
    if pad_with_silence {
        let silence = gst::ElementFactory::make("audiotestsrc")
            .property_from_str("wave", "silence")
            .build()
            .context("GStreamer element `audiotestsrc` is unavailable")?;
        pipeline.add(&silence)?;
        let pad = input()?;
        silence
            .static_pad("src")
            .expect("audiotestsrc src")
            .link(&pad)?;
    }
    Ok((driving, original))
}

/// Wait for the bus to report `kind` or an error, up to `timeout`.
pub fn wait_for(
    bus: &gst::Bus,
    kind: gst::MessageType,
    timeout: gst::ClockTime,
    what: &str,
) -> Result<()> {
    let message = bus
        .timed_pop_filtered(timeout, &[kind, gst::MessageType::Error])
        .ok_or_else(|| anyhow!("GStreamer did not {what} within {timeout}"))?;
    if let gst::MessageView::Error(error) = message.view() {
        return Err(anyhow!("GStreamer: {}", error_text(error)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AudioMode;

    #[test]
    fn audio_mode_falls_back_to_available_tracks() {
        use AudioMode::*;
        assert_eq!(Mix.resolve(true, true), Mix);
        assert_eq!(Mix.resolve(true, false), Driving);
        assert_eq!(Mix.resolve(false, true), Original);
        assert_eq!(Mix.resolve(false, false), None);
        assert_eq!(Driving.resolve(false, true), None);
        assert_eq!(Original.resolve(true, false), None);
        assert_eq!(Original.resolve(true, true), Original);
    }
}
