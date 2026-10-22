//! GStreamer integration for reactive-vcr: probing, audio decoding, the effect filter
//! element, real-time playback, app preview, and file rendering.
pub use gstreamer as gst;

pub mod analyze;
pub mod decode;
mod filter;
mod init;
pub mod media;
pub mod play;
pub mod preview;
pub mod probe;
pub mod render;
pub mod stream;
pub mod telemetry;

pub use filter::{Program, ReactiveFilter, frame_index};
pub use init::{init, main_thread};
pub use media::AudioMode;
