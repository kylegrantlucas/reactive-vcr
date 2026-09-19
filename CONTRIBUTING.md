# Contributing

Thanks for improving reactive-vcr. Focused bug fixes, performance improvements, documentation corrections, and new reusable mappings are welcome.

## Set up

Install a current stable Rust toolchain, FFmpeg with ffprobe, Python 3, and [just](https://just.systems/). Then clone the repository and run:

```sh
just build
just test
just lint
```

The end-to-end tests synthesize their own media. They must run with `ffmpeg` and `ffprobe` available; do not treat a skipped media test as a successful release check.

## Make a change

- Add a failing test before changing behavior.
- Keep the CLI and mapping format backward compatible within the `0.1.x` series.
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
