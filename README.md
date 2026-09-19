# reactive-vcr

[![CI](https://github.com/kylegrantlucas/reactive-vcr/actions/workflows/ci.yml/badge.svg)](https://github.com/kylegrantlucas/reactive-vcr/actions/workflows/ci.yml)
[![Demo](https://img.shields.io/badge/demo-watch-79f6dc)](https://kylegrantlucas.github.io/reactive-vcr/)
[![License: MIT](https://img.shields.io/badge/license-MIT-f7e26b)](LICENSE)

Make analog damage move to the music.

`reactive-vcr` renders an [ntsc-rs](https://github.com/valadaptive/ntsc-rs) VHS/NTSC effect whose settings respond to an audio file. A declarative TOML mapping connects RMS level, named frequency bands, and onset events to any supported ntsc-rs setting.

**[Watch the full interactive showcase](https://kylegrantlucas.github.io/reactive-vcr/)** — one clean source, eight treatments, and an original synthesized soundtrack.

## Requirements

- `ffmpeg` and `ffprobe` on `PATH`.
- A supported release archive, or a current stable Rust toolchain when building from source.
- Python 3 and [just](https://just.systems/) only for contributor and showcase recipes.

Install FFmpeg with Homebrew on macOS or your Linux distribution's package manager:

```sh
brew install ffmpeg
# Ubuntu / Debian
sudo apt install ffmpeg
```

## Install

Download the latest archive from [GitHub Releases](https://github.com/kylegrantlucas/reactive-vcr/releases/latest):

- `reactive-vcr-v0.1.0-aarch64-apple-darwin.tar.gz` for Apple Silicon macOS.
- `reactive-vcr-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` for x86_64 Linux with glibc 2.35 or newer.

Extract the archive and move `reactive-vcr` to a directory on `PATH`. The macOS binary is unsigned. If Gatekeeper blocks it after you verify `SHA256SUMS`, remove the downloaded-file quarantine attribute:

```sh
xattr -d com.apple.quarantine /path/to/reactive-vcr
```

To build from source instead:

```sh
git clone https://github.com/kylegrantlucas/reactive-vcr.git
cd reactive-vcr
cargo install --path . --locked
```

## Quick start

Render a clip with the included mapping:

```sh
reactive-vcr render \
  --video in.mp4 \
  --audio song.wav \
  --map examples/basic.toml \
  --output out.mp4
```

Use `--preview 0.25` while tuning a look. List every targetable effect parameter with `reactive-vcr params`, or inspect the audio features and final per-frame values before rendering:

```sh
reactive-vcr analyze \
  --audio song.wav \
  --fps 29.97 \
  --map examples/basic.toml \
  --output features.csv
```

The [showcase mappings](demo/maps/) provide eight more starting points.

## Commands

```text
reactive-vcr render  --video in.mp4 --audio song.wav --map fx.toml -o out.mp4
                     [--preview 0.5] [--start 12.0] [--duration 20.0]
                     [--audio-mode mix|driving|original|none]
                     [--ffmpeg-out-args "-c:v libx265 -crf 22"]

reactive-vcr analyze --audio song.wav --fps 29.97 --map fx.toml -o features.csv
                     [--start S] [--duration D]

reactive-vcr params  [--json]
```

`render` applies the selected audio timeline to the selected video timeline. Its default audio mode mixes the driving audio with the video's original audio. `driving`, `original`, and `none` select one track or silence instead.

The renderer resamples video timestamps to a constant frame rate derived from the first video stream. Variable-rate input may duplicate or drop frames, with duration rounded to a frame boundary. Pass the same rate to `analyze` when comparing its CSV with a render.

## Mapping files

```toml
[base]
preset = "presets/worn-vhs.json" # optional ntsc-rs GUI preset
seed = 42                        # optional deterministic noise seed

[audio]
bands.bass = [20, 200]
bands.high = [3000, 12000]
normalize = "percentile:98"      # or "peak"
onset_sensitivity = 1.5

[[modulate]]
target = "tracking_noise_wave_intensity"
source = "band:bass"             # rms | band:<name> | onset
range = [2.0, 30.0]
curve = "pow:2"                  # linear | pow:<k> | exp | log
attack_ms = 10
release_ms = 300
invert = false

[[modulate]]
target = "head_switching"
source = "onset"
threshold = 0.5
release_ms = 150
```

For each mapping, the renderer smooths the source, optionally inverts it, applies the response curve, and maps the result into the target range. Integer and enum targets round to a valid value. Boolean targets compare against `threshold`. Multiple mappings on one target add their offsets from the base value, then clamp the total to the parameter's legal range.

RMS and frequency bands use a 60 dB window below their peak or selected percentile. All bands share one reference, which preserves the difference between quiet and loud bands. Onsets are one-frame triggers; `release_ms` turns them into useful decays. Group children such as `tracking_noise_*` take effect only while their group toggle is enabled.

Preset paths are relative to the mapping file. Export a preset from the ntsc-rs GUI, place it beside the mapping or in a subdirectory, and reference it through `[base].preset`.

Validation reports independent mapping errors together, including unknown targets, undefined bands, malformed curves, incompatible target options, and out-of-range values. Output files may not alias any input, preset, symbolic link target, or Unix hard link.

## Reproducing the showcase

The public site contains no hand-edited or commercial media. Its Python generator synthesizes the soundtrack and source video, renders every mapping with the release binary, builds the comparison and sampler reels, creates posters, and verifies the finished artifact.

```sh
just showcase
just showcase-serve
```

The complete site is written to `target/demo-site/`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please report security issues through the private process in [SECURITY.md](SECURITY.md).

## License

reactive-vcr is licensed under the [MIT License](LICENSE). It uses the ntsc-rs core under ntsc-rs's MIT option. Binary archives include notices for all runtime dependencies.
