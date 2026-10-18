# Changelog

All notable changes to reactive-vcr appear here. This project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kylegrantlucas/reactive-vcr/releases/tag/v0.1.0
