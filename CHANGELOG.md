# Changelog

All notable changes to reactive-vcr appear here. This project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/kylegrantlucas/reactive-vcr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/kylegrantlucas/reactive-vcr/releases/tag/v0.1.0
