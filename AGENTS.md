# reactive-vcr

Audio-reactive ntsc-rs/bender-rs video effects driven by TOML mappings, shipped as a CLI and an egui desktop app on GStreamer (modeled on bender-rs). Human contributor guide: `CONTRIBUTING.md`.

## Layout
- `crates/reactive-vcr` core (no media I/O): audio features, mapping parse/compile/serialize, modulation `Schedule`, param catalog, `FrameProcessor`.
- `crates/reactive-vcr-gst`: GStreamer probe, audio decode, `ReactiveFilter` element, `play`, app `preview`, `render` jobs; integration tests synthesize media with GStreamer.
- `crates/reactive-vcr-cli` (binary `reactive-vcr`) and `crates/reactive-vcr-gui` (binary `reactive-vcr-gui`).
- `crates/reactive-vcr-wasm`: C-ABI cdylib of the core for the site's live deck (`wasm32-unknown-unknown`, no bindgen; `.cargo/config.toml` enables simd128, which ntsc-rs needs for correct chroma).
- `examples/` mapping TOMLs; `demo/` showcase site generator (`generate.py`, stdlib-only Python, uses FFmpeg), live deck (`engine.js`, `live.js`) + `demo/tests`.
- `scripts/macos-bundle.sh` builds `reactive-vcr.app` with the official GStreamer runtime bundled.
- `assets/` (gitignored) local source footage, e.g. `assets/100_4929-night-walk.mp4`; never commit media.

## Commands (run inside `nix develop` or via direnv `.envrc`)
- `just build` / `just test` (workspace tests + demo unittest) / `just lint` (clippy -D warnings + fmt check) / `just fmt`
- `just gui [args]`, `just cli <args>`, `just play <video> <audio> [map]`
- `just macos-bundle` (outside `nix develop`) builds `target/macos/reactive-vcr.app`
- `just demo` render synthetic clip to `target/demo.mp4`
- `just showcase` / `just showcase-check` / `just showcase-serve` build, verify, serve `target/demo-site/`; `just showcase-site` refreshes page, scripts, and wasm without rerendering video

## Invariants
- Keep CLI and mapping format backward compatible within 0.4.x.
- `play`, the app preview, and `render` must apply identical per-frame settings (`crates/reactive-vcr-cli/tests/cli.rs` checks byte parity).
- Add a failing test before behavior changes; skipped media tests are not a pass.
- Everything under `target/` is regenerable; showcase media is synthesized.
