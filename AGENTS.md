# reactive-vcr

Rust CLI that analyzes audio and drives VHS-style video effects from TOML mappings. Human contributor guide: `CONTRIBUTING.md`.

## Layout
- `src/` Rust crate (edition 2024), `tests/` integration tests (synthesize media with ffmpeg/ffprobe).
- `examples/` mapping TOMLs; `demo/` showcase site generator (`generate.py`, stdlib-only Python) + `demo/tests`.
- `assets/` (gitignored) local source footage, e.g. `assets/100_4929-night-walk.mp4`; never commit media.

## Commands (run inside `nix develop` or via direnv `.envrc`)
- `just build` / `just test` (cargo test --locked + demo unittest) / `just lint` (clippy -D warnings + fmt check) / `just fmt`
- `just demo` render synthetic clip to `target/demo.mp4`
- `just showcase` / `just showcase-check` / `just showcase-serve` build, verify, serve `target/demo-site/`

## Invariants
- Keep CLI and mapping format backward compatible within 0.2.x.
- Add a failing test before behavior changes; skipped media tests are not a pass.
- Everything under `target/` is regenerable; showcase media is synthesized.
