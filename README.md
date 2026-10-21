# reactive-vcr

[![CI](https://github.com/kylegrantlucas/reactive-vcr/actions/workflows/ci.yml/badge.svg)](https://github.com/kylegrantlucas/reactive-vcr/actions/workflows/ci.yml)
[![Demo](https://img.shields.io/badge/demo-watch-79f6dc)](https://klr.dev/reactive-vcr/)
[![License: MIT](https://img.shields.io/badge/license-MIT-f7e26b)](LICENSE)

Make analog damage move to the music.

`reactive-vcr` applies an [ntsc-rs](https://github.com/valadaptive/ntsc-rs) VHS/NTSC effect and [bender-rs](https://github.com/kylegrantlucas/bender-rs) circuit bends whose settings respond to music: a song you render against, or live sound from a microphone or your OBS stream. A declarative TOML mapping connects RMS level, named frequency bands, and onset events to any supported ntsc-rs or bender-rs setting.

**[Watch the full interactive showcase](https://klr.dev/reactive-vcr/)**: one clean source, eleven treatments, an original synthesized soundtrack, and a live deck that runs the effect in your browser on your own video, song, or camera.

## What's included

| Component | What it does |
|---|---|
| `reactive-vcr.app` / `reactive-vcr-gui` | Desktop app. Drop in a video and a song, watch the effect react with sound, shape the mapping with controls, save it, and render. |
| `reactive-vcr` | Command line: `render` a video, `play` it in real time, or `stream` your camera live with your microphone driving the effect. Also `analyze` and `params`. |
| OBS Studio plugin | Puts the effect on your stream. Add the **reactive-vcr** filter to a webcam, game capture, or any other source, and your mic or music drives it as you broadcast. |
| `reactive-vcr` crate | The effect core: audio features, live analysis, mappings, modulation, and the ntsc-rs/bender-rs frame processor. |
| `reactive-vcr-gst` crate | [GStreamer](https://gstreamer.freedesktop.org/) decoding, preview, playback, streaming, and rendering. |

## Install

Download the latest archive for your platform from [GitHub Releases](https://github.com/kylegrantlucas/reactive-vcr/releases/latest) and check it against `SHA256SUMS`.

### macOS (Apple Silicon)

`reactive-vcr-v0.5.0-aarch64-apple-darwin.tar.gz` contains `reactive-vcr.app` with GStreamer bundled inside, so nothing else needs installing. Move the app to `/Applications`. The app is ad-hoc signed, not notarized. If Gatekeeper blocks it, remove the download quarantine:

```sh
xattr -dr com.apple.quarantine /Applications/reactive-vcr.app
```

The command-line tool ships inside the app. Link it onto your `PATH`:

```sh
ln -s /Applications/reactive-vcr.app/Contents/MacOS/reactive-vcr /usr/local/bin/reactive-vcr
```

`GSTREAMER-NOTICE.txt` in the archive covers the bundled GStreamer libraries and their licenses.

### Linux (x86_64 and arm64)

`reactive-vcr-v0.5.0-<arch>-unknown-linux-gnu.tar.gz` contains `reactive-vcr` and `reactive-vcr-gui` for glibc 2.35 or newer. They use the system GStreamer:

```sh
sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-gl
```

### From source

Install GStreamer 1.20 or newer with its development headers (`brew install gstreamer pkgconf`, or `pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libwayland-dev libxkbcommon-dev` plus the plugin packages above), then:

```sh
git clone https://github.com/kylegrantlucas/reactive-vcr.git
cd reactive-vcr
cargo install --locked --path crates/reactive-vcr-cli
cargo install --locked --path crates/reactive-vcr-gui
```

### OBS Studio plugin

Put reactive-vcr on your stream with the plugin for OBS Studio 30 or newer on macOS (Apple Silicon), Windows (x64), and Linux (x86_64 and arm64). Download `reactive-vcr-obs-v0.5.0-<target>.tar.gz` for your system (`.zip` on Windows). The plugin brings its own copy of the effect, so there is nothing else to install.

- macOS: move `reactive-vcr.plugin` to `~/Library/Application Support/obs-studio/plugins/`. It is ad-hoc signed; if OBS will not load it, run `xattr -dr com.apple.quarantine` on the moved plugin.
- Linux: copy the `reactive-vcr` folder to `~/.config/obs-studio/plugins/`.
- Windows: copy the `reactive-vcr` folder to `C:\ProgramData\obs-studio\plugins\`.

From source, `just obs-install` builds the plugin and installs it for the current user on macOS or Linux.

## Desktop app

Open `reactive-vcr.app`, or run `reactive-vcr-gui [video] [audio] [mapping.toml]`.

1. Drop a video and a song on the window, or use **Open video…** and **Open audio…**. A video's own soundtrack can drive the effect too.
2. Press play. The preview applies the effect to every frame in sync with the sound, and the meters under it show the audio features and each modulated setting.
3. Shape the look in the left panel: frequency bands and onset sensitivity, one card per modulation (target, source, range or threshold, curve, attack, release, invert), and the unmodulated base look for every ntsc-rs and bender-rs setting. Changes show immediately, even while paused.
4. **Save mapping…** writes the TOML file the command line uses, and **Render…** writes an H.264 MP4 or a lossless FFV1 MKV in the background.

The app remembers the current mapping and its settings between launches.

## Go live

reactive-vcr also runs live. Point a camera at the stage and let the room drive the tape: with the included mapping, the picture tears on the kick and fills with snow when the crowd gets loud. Mappings are the same files everywhere, so a look you tuned in the desktop app goes straight on air.

### On your stream, in OBS

Install the [OBS plugin](#obs-studio-plugin), then add the **reactive-vcr** filter to any source, such as a webcam, a game capture, a screen share, or a media clip. Choose a **Mapping file** and the **Audio source** that drives it, whether that is your mic, the music bed, or desktop audio, and the effect plays along with your show.

Change looks mid-show by saving the mapping: the new treatment appears within a quarter second and the stream never stops. If an edit has a mistake, the filter keeps the last good look and shows the problem in its properties. **Processing lines** trades sharpness for speed, and 480 is a good place to start. Mute the driving audio, or set it to None, and the picture settles into its quiet look. Overlays with transparency keep it.

### From a camera and a microphone

```sh
reactive-vcr stream --map examples/basic.toml
```

This opens a window with your camera running through the deck and your microphone driving it. Clap and watch it answer. Edit the mapping while it runs and the look changes live; press Q to stop. On macOS, the first run asks for camera and microphone access.

Any camera, microphone, or feed that GStreamer can open works as an input, and the result can go to a window or any GStreamer sink: see `--video-source`, `--audio-source`, `--video-sink`, and `--lines` (480 by default) under [Commands](#commands). The driving audio is not played back unless you set `--audio-sink`, so a microphone cannot feed back.

A camera or other live feed always shows its newest frame: when the effect falls behind, stale frames are dropped so the picture stays current. A file or a pipe plays every frame at its own rate, starting from when its first frame arrives, so a clip can stand in for the camera while you tune a look:

```sh
reactive-vcr stream --map examples/basic.toml \
  --video-source "filesrc location=clip.mp4 ! decodebin"
ffmpeg -re -i clip.mp4 -f yuv4mpegpipe - | reactive-vcr stream --map examples/basic.toml \
  --video-source "fdsrc fd=0 ! y4mdec"
```

Live mode reacts to what it has just heard and cannot look ahead the way `render` does. Levels adapt to the last 10 seconds of sound, and hits register one frame after they land.

## Command line

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

The [showcase mappings](demo/maps/) provide eleven more starting points, and [`examples/bender.toml`](examples/bender.toml) adds circuit bends ahead of the default VHS look.

### Live playback

`play` shows the effect in a window while the audio plays:

```sh
reactive-vcr play --video in.mp4 --audio song.wav --map examples/basic.toml
```

Each frame gets the same settings that `render` would give it, matched by the frame's timestamp. For a lossless source, the frames that `play` shows are byte-identical to the ones `render` writes once both are converted to the same YUV format. Saved edits to the mapping file or its presets apply while the video plays or sits paused. If an edit does not validate, `play` prints the problems and keeps the previous mapping.

| Key | Action |
|---|---|
| Space | Pause or resume |
| ← / → | Seek back or forward 5 s |
| Home | Restart at `--start` |
| Q / Esc | Quit |

Playback stops at the end of the selected range unless you pass `--loop`. The effect runs on the CPU, so use `--preview` to lower the resolution when large video cannot keep up. Bender stages that remember earlier frames, such as feedback and frame store, start empty again after each seek.

### Commands

```text
reactive-vcr render  --video in.mp4 --audio song.wav --map fx.toml -o out.mp4
                     [--preview 0.5] [--start 12.0] [--duration 20.0]
                     [--audio-mode mix|driving|original|none]
                     [--codec h264|ffv1] [--quantizer 18] [--quiet]

reactive-vcr play    --video in.mp4 --audio song.wav --map fx.toml
                     [--preview 0.5] [--start 12.0] [--duration 20.0]
                     [--audio-mode mix|driving|original|none] [--loop]
                     [--video-sink autovideosink] [--audio-sink autoaudiosink]

reactive-vcr stream  --map fx.toml [--lines 480]
                     [--video-source autovideosrc] [--audio-source autoaudiosrc]
                     [--video-sink autovideosink] [--audio-sink fakesink] [--quiet]

reactive-vcr analyze --audio song.wav --fps 29.97 --map fx.toml -o features.csv
                     [--start S] [--duration D]

reactive-vcr params  [--json]
```

`render` applies the selected audio timeline to the selected video timeline. Its default audio mode mixes the driving audio with the video's original audio. `driving`, `original`, and `none` select one track or silence instead. The output audio always lasts as long as the video.

The output extension picks the codec: `.mp4` writes H.264 and AAC with the index at the front for web playback, and `.mkv` writes FFV1 and FLAC. FFV1 converts the effect's frames to 4:4:4 YUV, then compresses them losslessly on every CPU core. Both codecs convert with the BT.601 matrix up to 576 lines and BT.709 above, and tag the output to match. `--codec` overrides the extension, and `--quantizer` sets H.264 quality (0 is lossless, 18 is the default, 51 is the lowest). Output resolution is the source size scaled by `--preview`, and the output keeps the source's pixel aspect ratio.

The renderer resamples video to a constant frame rate taken from the first video stream. Variable-rate input may duplicate or drop frames, with duration rounded to a frame boundary. Pass the same rate to `analyze` when comparing its CSV with a render.

`play` takes the same timeline and audio options. `--video-sink` and `--audio-sink` accept gst-launch descriptions, such as `"glimagesink"` or `"volume volume=0.5 ! autoaudiosink"`, and may start with caps to pick the format the sink receives, such as `"video/x-raw,format=I420 ! x264enc ! mpegtsmux ! udpsink host=… port=5000"` for an H.264 stream most players can decode.

## Mapping files

```toml
[base]
preset = "presets/worn-vhs.json" # optional ntsc-rs GUI preset
seed = 42                        # optional deterministic noise seed
ntsc = true                      # false skips ntsc-rs and renders bends only

[base.set]
snow_intensity = 0.4             # any ntsc-rs setting; enum values by label or index
vhs_tape_speed = "LP (long play)"

[bender]
preset = "bends.json"            # optional bender-rs preset
seed = 3                         # optional bend seed; overrides the preset
order = "before"                 # run bends "before" or "after" ntsc-rs

[bender.set]
order = ["hold", "sync"]         # bender-rs stage order; listed stages run first
"sync.enabled" = true            # any bender-rs setting id; enum values by name
"hold.enabled" = true
"invert.mode" = "solarize"

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

bender-rs targets are named `bender.<group>.<setting>`, such as `bender.sync.v_roll`; `reactive-vcr params` lists all of them. Each `bender.<group>.*` setting takes effect only while `bender.<group>.enabled` is on, either from the preset, `[bender.set]`, or a modulation. `[bender.set]` values apply after the preset and are checked strictly: out-of-range values are errors, not clamped. Quoted (`"sync.enabled" = true`), dotted (`sync.enabled = true`), and nested (`[bender.set.sync]`) keys are equivalent.

`[bender] order` places the whole bender-rs chain relative to ntsc-rs. `[bender.set] order` reorders the bender-rs stages themselves, overriding the preset's `order`: the listed stages run first and the rest follow in bender-rs's default order (`sensor, retina, isp, tape, sync, clock, data, dct, mosh, hold, counter, address_a, address_b, colorize, invert, rail, osc, burst, bleed, composite, scan, feedback`). It takes an array of stage names or a comma-separated string, as in `bender --set order=hold,sync`. Stage order is fixed for the whole render and cannot be modulated.

`[base.set]` sets ntsc-rs values after the preset, the same way `[bender.set]` does for bender-rs. Unknown names and values that don't fit a setting are errors. Preset paths are relative to the mapping file. Export a preset from the ntsc-rs GUI, place it beside the mapping or in a subdirectory, and reference it through `[base].preset`; bender-rs presets go in `[bender].preset`.

Validation reports independent mapping errors together, including unknown targets, undefined bands, malformed curves, incompatible target options, out-of-range values, and ntsc-rs options that have no effect while `[base] ntsc = false`. Output files may not alias any input, preset, symbolic link target, or Unix hard link.

## Reproducing the showcase

The public site contains no hand-edited or commercial media. Its Python generator synthesizes the soundtrack and source video with FFmpeg, renders every mapping with the release command-line tool, builds the comparison and sampler reels, creates posters, and verifies the finished artifact.

The site's live deck runs the core in the browser. `crates/reactive-vcr-wasm` compiles it to a dependency-free WebAssembly module; the page decodes the soundtrack, analyzes it the way `render` does, and processes each frame of the picture with the scheduled settings. Its output matches the native renderer to within one level per channel.

```sh
just showcase
just showcase-serve
```

The complete site is written to `target/demo-site/`. `just showcase-site` refreshes the page, scripts, and WebAssembly engine without rerendering video. Building the engine needs the `wasm32-unknown-unknown` Rust target (`rustup target add wasm32-unknown-unknown`; `nix develop` includes it).

## Development

Run everything inside `nix develop` (or direnv), which provides Rust, GStreamer with all plugin sets, FFmpeg for the showcase, and `just`.

```sh
just build                    # whole workspace
just test                     # Rust tests and the showcase generator's tests
just lint                     # clippy -D warnings and rustfmt
just gui [video audio map]    # run the desktop app
just cli render ...           # run the command line
just stream --map fx.toml     # stream camera and microphone through the effect
just obs-install              # build the OBS plugin and install it for this user
just macos-bundle             # macOS: build target/macos/reactive-vcr.app with GStreamer bundled (run outside nix develop)
```

### Releasing

Bump `[workspace.package] version` in `Cargo.toml`, add the release to `CHANGELOG.md`, merge, then push a matching tag (`git tag v0.5.0 && git push origin v0.5.0`). The release workflow builds the macOS app, the Linux archives, and the OBS plugin for macOS, Linux, and Windows, and publishes them with `SHA256SUMS`. Run the workflow manually for a dry run that uploads the archives without publishing.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please report security issues through the private process in [SECURITY.md](SECURITY.md).

## License

reactive-vcr is licensed under the [MIT License](LICENSE). It uses the ntsc-rs core under ntsc-rs's MIT option and the bender-rs core under its MIT option. Release archives include notices for all Rust dependencies, and the macOS app includes the notice for its bundled GStreamer runtime.

The OBS plugin (`crates/reactive-vcr-obs`) links libobs and is licensed under the [GNU GPL, version 2 or later](crates/reactive-vcr-obs/COPYING); its archives include that license. The rest of the project stays MIT.
