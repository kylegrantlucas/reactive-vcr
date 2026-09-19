set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default: build

build:
    cargo build

test:
    cargo test --locked
    python3 -m unittest discover -s demo/tests -v

lint:
    cargo clippy --locked --all-targets -- -D warnings
    cargo fmt --check

fmt:
    cargo fmt

params:
    cargo run --quiet -- params

analyze audio map="examples/basic.toml" fps="30" out="target/features.csv":
    cargo run --release -- analyze --audio {{audio}} --map {{map}} --fps {{fps}} -o {{out}}

# Render examples/basic.toml against a synthetic 4s test clip (needs ffmpeg)
demo out="target/demo.mp4":
    mkdir -p target
    ffmpeg -v error -y -f lavfi -i "testsrc2=size=640x360:rate=30:duration=4" -pix_fmt yuv420p target/demo-in.mp4
    ffmpeg -v error -y -f lavfi -i "sine=frequency=110:duration=4,volume=0.8,tremolo=f=2:d=0.9" target/demo-audio.wav
    cargo run --release --locked -- render --video target/demo-in.mp4 --audio target/demo-audio.wav --map examples/basic.toml -o {{out}}

# Rebuild every video and the complete static site.
showcase:
    cargo build --release --locked
    python3 demo/generate.py build

# Recheck an existing target/demo-site without rerendering it.
showcase-check:
    python3 demo/generate.py verify

# Preview the built site at http://localhost:8000.
showcase-serve port="8000":
    python3 -m http.server {{port}} --directory target/demo-site
