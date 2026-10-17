use anyhow::{Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::OnceLock;

/// Initialize GStreamer once and register the `reactivevcrfilter` element.
pub fn init() -> Result<()> {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            gst::init().map_err(|error| error.to_string())?;
            gst::Element::register(
                None,
                "reactivevcrfilter",
                gst::Rank::NONE,
                crate::ReactiveFilter::static_type(),
            )
            .map_err(|error| error.to_string())
        })
        .clone()
        .map_err(|error| anyhow!("initializing GStreamer: {error}"))
}

/// Run `body` while the process main thread serves macOS's NSApplication, which video sinks
/// and hardware decoders need there. Call it from `main`; elsewhere it runs `body` directly.
pub fn main_thread<T>(body: impl FnOnce() -> T + Send) -> T {
    #[cfg(target_os = "macos")]
    {
        gst::macos_main(body)
    }
    #[cfg(not(target_os = "macos"))]
    {
        body()
    }
}
