//! reactive-vcr core: audio features, mappings, modulation, and the ntsc-rs/bender-rs frame
//! processor. Media decoding, playback, and rendering live in `reactive-vcr-gst`.
pub mod analyze;
pub mod audio;
pub mod curve;
pub mod effect;
pub mod mapping;
pub mod modulate;
pub mod params;
pub mod smooth;
pub mod timeline;
