# Changelog

All notable changes to reactive-vcr appear here. This project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `check` command: compiles a mapping exactly as `stream` and `render` do, without opening any media, and exits 0 when it is valid, 1 when it is invalid, and 2 when it can't be read. `--json` prints one line, `{"ok":true}` or `{"ok":false,"errors":[...]}` with one entry per problem.
- `stream --telemetry HOST:PORT --telemetry-id ID [--telemetry-hz 15]`: UDP JSON telemetry for a control panel, with a `start` message, up to 15 `frame` messages a second (frames processed and dropped, frame rate, effect time, audio features, every modulated target's value, and the latest hot-reload error), and an `error` message before a fatal exit. Sending never blocks the video.
- Core: `live::LiveProgram::features`, `band_names`, and `targets` report the latest frame's features and applied target values, and compile errors carry each problem as `mapping::MappingProblems`.
- `stream --gate-db <dBFS>`: a noise gate on the driving audio. Below the threshold the input reads as silence, with every feature at 0 and no hits, and the quiet stretch stays out of the 10-second level reference, so an idle mixer's hiss no longer drives the picture and the next song reads as loud as the last. The gate opens above the threshold and closes 3 dB below it.
- Telemetry frame messages carry `level_db`, the driving audio's level in dBFS to a tenth (`null` at digital silence), and `gated`, so a control panel can show an input meter and set the gate just above the noise floor.
- Core: `live::LiveAnalyzer::set_gate` and `live::LiveProgram::set_gate`, with `gated` and `level_db`; `live::GATE_HYSTERESIS_DB` is 3 dB.

### Changed

- The showcase site lives at https://klr.dev/reactive-vcr/: its canonical and social-card URLs, the README links, and the crate homepage point there, and the footer credits Kyle Lucas-Rodriguez.
- Per-frame settings skip composite, luma, and chroma noise passes when their intensity is exactly zero, without cloning extra settings or changing nonzero noise seeds. Mapping settings and reported modulation values are preserved.
- `stream` omits the unused audio monitor branch, tee, and queues when `--audio-sink` is exactly the default `fakesink`. Audio analysis keeps its existing caps and source-driven pacing; explicitly configured audio sinks retain their monitor branch.
- Bender frames use its packed RGB interface directly, removing the two RGBA scratch frames and RGB/RGBA copies. Colour conversion, stage state, seeds, and intermediate RGB rounding remain unchanged.

### Fixed

- `stream` drains downstream queues, encoders, and muxers when finite video ends, stopping live audio and waiting for pipeline EOS. It previously stopped at the effect's EOS notification and could truncate encoded files.
- `stream` accepts file sources such as `filesrc location=clip.mp4 ! decodebin`, `uridecodebin`, or a demuxer and decoder. They failed at once with `not-linked`, because the element after the demuxer or decoder was taken as the source's input.
- `stream` plays every frame of a file, pipe, or other non-live source at its own rate, starting when its first frame arrives. Only 3 of 300 frames of a non-live `videotestsrc` reached the effect, and a pipe from `ffmpeg -re` lost most of its frames to a synced sink. Cameras and other live sources still drop stale frames when the effect falls behind.
- `--video-sink` and `--audio-sink` may start with caps, such as `video/x-raw,format=I420 ! x264enc ! ...`, so `x264enc` can encode 4:2:0 H.264 that most players decode instead of High 4:4:4 from the effect's RGB output.
- Hot reload notices a mapping replaced by renaming another file over it even when both have the same size and modification time, as on filesystems with coarse timestamps, on every platform including Windows (it compares the files' contents), and no longer misses a replacement that lands while the mapping is being read.

## [0.5.0] - 2026-10-18

### Added

- `stream` command: live video from any GStreamer source (the camera by default) through the effect in real time, driven by live audio (the microphone by default), shown in a window or sent to any GStreamer sink, with live mapping reload.
- OBS Studio plugin (OBS 30 or newer; macOS Apple Silicon, Linux x86_64 and arm64, Windows x64): a **reactive-vcr** video filter for any source, driven by an OBS audio source you pick. It is GPL-2.0-or-later because it links libobs; the rest of the project stays MIT.
- Core: `live::LiveProgram` and `live::LiveAnalyzer` for causal live analysis (a trailing 2048-sample window, levels normalized over the last 10 seconds, onsets one frame late), `watch::MappingWatcher` for hot reload, and `FrameProcessor::process_bgra`, which keeps each pixel's alpha.

### Changed

- Core API: `Modulator` owns its state. `Modulator::new` no longer takes a feature timeline, `step` takes a closure that yields each source's value, and `effect` replaces `effect_for_frame`. Offline output is unchanged.

## [0.4.1] - 2026-10-17

### Added

- Live deck on the showcase site: the core runs in the browser as WebAssembly and bends the demo clip, your video, or your camera to the demo beat or your song, with the showcase mappings as presets. Nothing is uploaded.
- `mapping::compile_with` compiles a mapping with preset files supplied by the caller instead of the filesystem.

### Changed

- Faster renders: FFV1 encodes on every CPU core with 16 slices, the effect runs on a separate thread from conversion and encoding for both FFV1 and H.264, and source conversion and scaling use every core. A 10-second 1080p FFV1 render on an M2 Max went from 32 s to 22 s.
- FFV1 output is 4:4:4 YUV instead of RGB (bgr0), which is smaller. Frames convert with BT.601 up to 576 lines and BT.709 above, and both FFV1 and H.264 output carry matching colour tags.
- Sliced FFV1 input decodes on every core.

### Fixed

- Renders and playback keep the source's pixel aspect ratio instead of resetting it to square pixels, so anamorphic video keeps its shape.

## [0.4.0] - 2026-10-17

### Added

- Desktop app (`reactive-vcr.app` on macOS, `reactive-vcr-gui` on Linux). Drop in a video and a song to preview the effect live with sound, edit bands, modulations, and every base setting, see per-frame audio and target meters, save the mapping, and queue renders.
- `play` command: real-time playback in a GStreamer window with the same per-frame settings as `render`, keyboard seeking and pausing, and live reload of mapping and preset edits.
- `[base.set]` mapping table for inline ntsc-rs settings, mirroring `[bender.set]`.
- `render --codec h264|ffv1` and `--quantizer`.
- macOS release archive with an app bundle that includes the GStreamer runtime, and a Linux arm64 archive.

### Changed

- GStreamer replaces FFmpeg for probing, decoding, and encoding, so FFmpeg is no longer needed. `.mp4` output is H.264 and AAC with the index at the front; `.mkv` output is lossless FFV1 and FLAC. Decoders and resamplers differ from FFmpeg's, so renders and analysis values are close to, not identical with, 0.3.
- The project is a Cargo workspace: `reactive-vcr` (core), `reactive-vcr-gst`, `reactive-vcr-cli` (the `reactive-vcr` binary), and `reactive-vcr-gui`.

### Removed

- `render --ffmpeg-out-args`. Use `--codec` and `--quantizer` instead.

## [0.3.0] - 2026-10-17

### Added

- bender-rs 0.2.0 circuits: sensor, retina, isp, tape, composite, dct, mosh, counter, colorize, scan, and feedback, plus the new settings on existing stages. All 157 bender-rs settings are modulation targets.
- `[bender.set] order` reorders the bender-rs stages, from an array of stage names or a comma-separated string.
- bender-rs version 2 presets, including their stage `order`.

### Changed

- Upgraded bender-rs to 0.2.0. Its sync, clock, data, hold, rail, osc, and burst stages are retuned, so existing bender mappings render differently.

## [0.2.0] - 2026-10-17

### Added

- bender-rs circuit bends as a second effect engine. All 57 bender-rs settings are modulation targets named `bender.<group>.<setting>`.
- `[bender]` mapping section with a bender-rs preset, seed, strict inline `set` values, and `order = "before" | "after"` relative to ntsc-rs.
- `[base] ntsc = false` to skip ntsc-rs and render bends only.
- `examples/bender.toml` mapping.
- Three bender showcase treatments (Rolling sync, Frame store, Burst drift) and a 3×4 comparison grid.

## [0.1.0] - 2026-09-19

### Added

- Audio-reactive offline rendering through the ntsc-rs effect engine.
- RMS, named frequency-band, and spectral-flux onset sources.
- Declarative TOML mappings with curves, inversion, attack, release, thresholds, and deterministic seeds.
- `render`, `analyze`, and `params` commands.
- Mixed, driving, original, and silent output-audio modes.
- Constant-frame-rate handling for variable-rate video input.
- Public showcase with eight reproducible treatments.
- Apple Silicon macOS and x86_64 Linux release archives.

[Unreleased]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kylegrantlucas/reactive-vcr/releases/tag/v0.1.0
