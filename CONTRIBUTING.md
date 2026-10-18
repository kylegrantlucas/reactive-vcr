# Contributing

Thanks for improving reactive-vcr. Focused bug fixes, performance improvements, documentation corrections, and new reusable mappings are welcome.

## Set up

Install a current stable Rust toolchain with the `wasm32-unknown-unknown` target (for the showcase's live deck), GStreamer with its development headers and the base, good, bad, ugly and libav plugin sets, FFmpeg (used by the showcase generator), Python 3, and [just](https://just.systems/). `nix develop` provides all of them. Then clone the repository and run:

```sh
just build
just test
just lint
```

The integration tests synthesize their own media with GStreamer and need the base, good, ugly, and libav plugin sets; `nix develop` provides them. Do not treat a skipped or failing media test as a successful release check.

## Make a change

- Add a failing test before changing behavior.
- Keep the CLI and mapping format backward compatible within the `0.4.x` series.
- Run `cargo fmt` before submitting Rust changes.
- Keep generated videos, audio, and `target/` artifacts out of Git.
- Use original or redistributable material in mappings and showcase changes.

To rebuild and inspect the public site:

```sh
just showcase
just showcase-check
just showcase-serve
```

## Submit

Open a focused pull request that explains the user-visible change, its tests, and any compatibility implications. Confirm `just test`, `just lint`, and `git diff --check` before requesting review.
