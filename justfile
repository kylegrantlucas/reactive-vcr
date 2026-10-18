set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default: build

build:
    cargo build --workspace

test:
    cargo test --workspace --locked
    python3 -m unittest discover -s demo/tests -v

lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo fmt --all --check

fmt:
    cargo fmt --all

# Run the desktop app.
gui *ARGS:
    cargo run --release --locked -p reactive-vcr-gui -- {{ARGS}}

# Run the command-line tool.
cli *ARGS:
    cargo run --release --locked -p reactive-vcr-cli -- {{ARGS}}

params:
    cargo run --quiet --locked -p reactive-vcr-cli -- params

# Play a clip in a window with live mapping reload
play video audio map="examples/basic.toml":
    cargo run --release --locked -p reactive-vcr-cli -- play --video {{video}} --audio {{audio}} --map {{map}}

# Stream live video through the effect, driven by live audio (defaults: camera and microphone)
stream *ARGS:
    cargo run --release --locked -p reactive-vcr-cli -- stream {{ARGS}}

# Build the OBS Studio plugin and install it into this user's OBS plugins directory
obs-install:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin)
        target=aarch64-apple-darwin
        plugins_dir="$HOME/Library/Application Support/obs-studio/plugins"
        installed=reactive-vcr.plugin
        ;;
      Linux)
        target="$(uname -m)-unknown-linux-gnu"
        plugins_dir="$HOME/.config/obs-studio/plugins"
        installed=reactive-vcr
        ;;
      *)
        echo "obs-install supports macOS and Linux" >&2
        exit 1
        ;;
    esac
    cargo build --release --locked -p reactive-vcr-obs --target "$target"
    mkdir -p "$plugins_dir"
    rm -rf "${plugins_dir:?}/$installed"
    scripts/obs-plugin.sh "$target" "$plugins_dir"
    echo "installed $plugins_dir/$installed"

analyze audio map="examples/basic.toml" fps="30" out="target/features.csv":
    cargo run --release --locked -p reactive-vcr-cli -- analyze --audio {{audio}} --map {{map}} --fps {{fps}} -o {{out}}

# Render examples/basic.toml against a synthetic 4s test clip (ffmpeg makes the inputs)
demo out="target/demo.mp4":
    mkdir -p target
    ffmpeg -v error -y -f lavfi -i "testsrc2=size=640x360:rate=30:duration=4" -pix_fmt yuv420p target/demo-in.mp4
    ffmpeg -v error -y -f lavfi -i "sine=frequency=110:duration=4,volume=0.8,tremolo=f=2:d=0.9" target/demo-audio.wav
    cargo run --release --locked -p reactive-vcr-cli -- render --video target/demo-in.mp4 --audio target/demo-audio.wav --map examples/basic.toml -o {{out}}

# Build target/macos/reactive-vcr.app with the official GStreamer runtime bundled.
# Run it outside `nix develop`, with a plain pkg-config on PATH (or set PKG_CONFIG).
macos-bundle DEST="target/macos":
    scripts/macos-bundle.sh {{DEST}}

# Rebuild every video and the complete static site.
showcase: showcase-wasm
    cargo build --release --locked -p reactive-vcr-cli
    python3 demo/generate.py build

# Build the live demo's WebAssembly engine.
showcase-wasm:
    cargo build --release --locked -p reactive-vcr-wasm --target wasm32-unknown-unknown

# Refresh the page, scripts, and engine of an existing target/demo-site without rerendering video.
showcase-site: showcase-wasm
    python3 demo/generate.py site

# Recheck an existing target/demo-site without rerendering it.
showcase-check:
    python3 demo/generate.py verify

# Preview the built site at http://localhost:8000.
showcase-serve port="8000":
    python3 -m http.server {{port}} --directory target/demo-site
