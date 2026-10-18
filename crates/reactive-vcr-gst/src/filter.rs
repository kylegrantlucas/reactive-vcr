//! Video filter that applies an effect to each frame: from a precomputed schedule by stream
//! time, or from live audio analysis frame by frame.

use gstreamer as gst;
use gstreamer::glib;
use gstreamer::subclass::prelude::*;
use gstreamer_base as gst_base;
use gstreamer_video as gst_video;
use reactive_vcr::audio::features::FeatureTimeline;
use reactive_vcr::effect::EffectSettings;
use reactive_vcr::live::LiveProgram;
use reactive_vcr::mapping::CompiledMapping;
use reactive_vcr::modulate::Schedule;
use reactive_vcr::params::ParamCatalog;
use reactive_vcr::timeline::Selection;
use std::sync::Arc;

/// Application message carrying a key pressed in the video window (`key` field).
pub const KEY_MESSAGE: &str = "reactive-vcr-key";
/// Application message posted when the video stream reaches its end.
pub const VIDEO_EOS_MESSAGE: &str = "reactive-vcr-video-eos";

/// Everything the filter needs to choose and apply a frame's effect.
pub struct Program {
    pub catalog: Arc<ParamCatalog>,
    pub schedule: Schedule,
    /// Source time of schedule frame 0, in seconds.
    pub start_s: f64,
    pub fps: f64,
}

impl Program {
    pub fn new(
        catalog: Arc<ParamCatalog>,
        map: &CompiledMapping,
        features: &FeatureTimeline,
        selection: &Selection,
    ) -> Self {
        Self {
            schedule: Schedule::build(&catalog, map, features, selection.fps()),
            catalog,
            start_s: selection.start_s,
            fps: selection.fps(),
        }
    }

    /// The schedule frame and its effect for a source stream time.
    pub fn effect_at(&self, stream_time: gst::ClockTime) -> (usize, EffectSettings) {
        let frame = frame_index(stream_time, self.start_s, self.fps);
        (frame, self.schedule.effect(&self.catalog, frame))
    }
}

/// Where the filter gets each frame's effect.
#[derive(Clone)]
pub(crate) enum Driver {
    /// Precomputed schedule, looked up by stream time.
    Schedule(Arc<Program>),
    /// Live analysis, advanced once per frame.
    Live(Arc<parking_lot::Mutex<LiveProgram>>),
}

/// Schedule frame for a source stream time, matching `render`'s constant-rate frame numbering.
pub fn frame_index(stream_time: gst::ClockTime, start_s: f64, fps: f64) -> usize {
    let offset_s = stream_time.nseconds() as f64 / 1e9 - start_s;
    (offset_s * fps).round().max(0.0) as usize
}

glib::wrapper! {
    pub struct ReactiveFilter(ObjectSubclass<imp::ReactiveFilter>)
        @extends gst_video::VideoFilter, gst_base::BaseTransform, gst::Element, gst::Object;
}

impl ReactiveFilter {
    pub fn new(program: Arc<Program>) -> Self {
        let filter: Self = glib::Object::new();
        filter.set_program(program);
        filter
    }

    /// A filter driven by live analysis; its frame rate follows the negotiated caps.
    pub fn new_live(live: Arc<parking_lot::Mutex<LiveProgram>>) -> Self {
        let filter: Self = glib::Object::new();
        *filter.imp().driver.lock() = Some(Driver::Live(live));
        filter
    }

    /// Swap the schedule; the next frame uses it.
    pub fn set_program(&self, program: Arc<Program>) {
        *self.imp().driver.lock() = Some(Driver::Schedule(program));
    }

    pub fn frames_processed(&self) -> u64 {
        self.imp()
            .processed
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Drop frames at or past `frames` in the schedule, so output stops at the selection.
    pub fn limit_frames(&self, frames: usize) {
        self.imp()
            .frame_limit
            .store(frames, std::sync::atomic::Ordering::Relaxed);
    }
}

mod imp {
    use super::{Driver, KEY_MESSAGE, VIDEO_EOS_MESSAGE};
    use gstreamer as gst;
    use gstreamer::glib;
    use gstreamer::prelude::*;
    use gstreamer::subclass::prelude::*;
    use gstreamer_base::prelude::*;
    use gstreamer_base::subclass::BaseTransformMode;
    use gstreamer_base::subclass::prelude::*;
    use gstreamer_video as gst_video;
    use gstreamer_video::prelude::*;
    use gstreamer_video::subclass::prelude::*;
    use parking_lot::Mutex;
    use reactive_vcr::effect::FrameProcessor;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    #[derive(Default)]
    pub struct ReactiveFilter {
        pub(super) driver: Mutex<Option<Driver>>,
        processor: Mutex<Option<FrameProcessor>>,
        pub(super) processed: AtomicU64,
        /// Frames at or past this schedule index are dropped; 0 keeps every frame.
        pub(super) frame_limit: AtomicUsize,
    }

    impl ReactiveFilter {
        fn post(&self, name: &str, key: Option<&str>) {
            let mut structure = gst::Structure::builder(name);
            if let Some(key) = key {
                structure = structure.field("key", key);
            }
            let message = gst::message::Application::builder(structure.build())
                .src(&*self.obj())
                .build();
            // Fails only without a bus, when nobody is listening anyway.
            let _ = self.obj().post_message(message);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReactiveFilter {
        const NAME: &'static str = "ReactiveVcrFilter";
        type Type = super::ReactiveFilter;
        type ParentType = gst_video::VideoFilter;
    }

    impl ObjectImpl for ReactiveFilter {}

    impl GstObjectImpl for ReactiveFilter {}

    static METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
        gst::subclass::ElementMetadata::new(
            "reactive-vcr filter",
            "Filter/Effect/Video",
            "Applies audio-reactive ntsc-rs and bender-rs effects",
            "reactive-vcr",
        )
    });

    static TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
        let caps = gst_video::VideoCapsBuilder::new()
            .format(gst_video::VideoFormat::Rgb)
            .build();
        [
            ("src", gst::PadDirection::Src),
            ("sink", gst::PadDirection::Sink),
        ]
        .into_iter()
        .map(|(name, direction)| {
            gst::PadTemplate::new(name, direction, gst::PadPresence::Always, &caps)
                .expect("valid pad template")
        })
        .collect()
    });

    impl ElementImpl for ReactiveFilter {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            Some(&METADATA)
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            &TEMPLATES
        }
    }

    impl BaseTransformImpl for ReactiveFilter {
        const MODE: BaseTransformMode = BaseTransformMode::NeverInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            *self.processor.lock() = None;
            Ok(())
        }

        fn sink_event(&self, event: gst::Event) -> bool {
            // bender-rs restarts temporal stages itself when frame numbers jump after a seek.
            if let gst::EventView::Eos(_) = event.view() {
                self.post(VIDEO_EOS_MESSAGE, None);
            }
            self.parent_sink_event(event)
        }

        fn src_event(&self, event: gst::Event) -> bool {
            if let Ok(gst_video::NavigationEvent::KeyPress { key, .. }) =
                gst_video::NavigationEvent::parse(&event)
            {
                self.post(KEY_MESSAGE, Some(&key));
                return true;
            }
            self.parent_src_event(event)
        }
    }

    impl VideoFilterImpl for ReactiveFilter {
        fn set_info(
            &self,
            incaps: &gst::Caps,
            in_info: &gst_video::VideoInfo,
            outcaps: &gst::Caps,
            out_info: &gst_video::VideoInfo,
        ) -> Result<(), gst::LoggableError> {
            if let Some(Driver::Live(live)) = self.driver.lock().as_ref() {
                let fps = in_info.fps();
                if fps.numer() > 0 {
                    live.lock().set_fps(fps.numer() as f64 / fps.denom() as f64);
                }
            }
            self.parent_set_info(incaps, in_info, outcaps, out_info)
        }

        fn transform_frame(
            &self,
            in_frame: &gst_video::VideoFrameRef<&gst::BufferRef>,
            out_frame: &mut gst_video::VideoFrameRef<&mut gst::BufferRef>,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let driver = self
                .driver
                .lock()
                .clone()
                .ok_or(gst::FlowError::NotNegotiated)?;
            let (frame, effect, chain) = match driver {
                Driver::Schedule(program) => {
                    let pts = in_frame.buffer().pts().ok_or(gst::FlowError::Error)?;
                    let segment = self.obj().segment();
                    let stream_time = segment
                        .downcast_ref::<gst::ClockTime>()
                        .and_then(|segment| segment.to_stream_time(pts))
                        .unwrap_or(pts);
                    let (frame, effect) = program.effect_at(stream_time);
                    let limit = self.frame_limit.load(Ordering::Relaxed);
                    if limit != 0 && frame >= limit {
                        return Ok(gstreamer_base::BASE_TRANSFORM_FLOW_DROPPED);
                    }
                    (frame, effect, program.schedule.chain())
                }
                Driver::Live(live) => {
                    let mut live = live.lock();
                    let (frame, effect) = live.next_frame();
                    (frame, effect, live.chain())
                }
            };

            let width = in_frame.width() as usize;
            let height = in_frame.height() as usize;
            let mut processor = self.processor.lock();
            let processor = match processor.as_mut() {
                Some(current) if current.fits(width, height, chain) => current,
                _ => processor.insert(FrameProcessor::new(width, height, chain)),
            };
            let in_stride = in_frame.plane_stride()[0] as usize;
            let out_stride = out_frame.plane_stride()[0] as usize;
            let input = in_frame.plane_data(0).map_err(|_| gst::FlowError::Error)?;
            let output = out_frame
                .plane_data_mut(0)
                .map_err(|_| gst::FlowError::Error)?;
            processor.process_strided(&effect, frame, input, in_stride, output, out_stride);
            self.processed.fetch_add(1, Ordering::Relaxed);
            Ok(gst::FlowSuccess::Ok)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: f64) -> gst::ClockTime {
        gst::ClockTime::from_nseconds((seconds * 1e9).round() as u64)
    }

    #[test]
    fn frame_index_counts_from_the_selected_start() {
        let fps = 30_000.0 / 1001.0;
        assert_eq!(frame_index(at(12.0), 12.0, fps), 0);
        assert_eq!(frame_index(at(12.0 + 7.0 / fps), 12.0, fps), 7);
        assert_eq!(frame_index(at(12.0 + 300.0 / fps), 12.0, fps), 300);
    }

    #[test]
    fn frame_index_tolerates_container_timestamp_rounding() {
        // MP4 timescales round each frame time; nearby timestamps keep their frame.
        assert_eq!(frame_index(at(10.0 / 30.0 - 0.0004), 0.0, 30.0), 10);
        assert_eq!(frame_index(at(10.0 / 30.0 + 0.0004), 0.0, 30.0), 10);
    }

    #[test]
    fn frames_before_the_start_use_frame_zero() {
        assert_eq!(frame_index(at(4.5), 5.0, 30.0), 0);
        assert_eq!(frame_index(gst::ClockTime::ZERO, 5.0, 30.0), 0);
    }
}
