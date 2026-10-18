#!/usr/bin/env python3
"""Build and verify the reactive-vcr public showcase."""

import argparse
import array
import concurrent.futures
from dataclasses import dataclass
import html
from html.parser import HTMLParser
import json
import math
from pathlib import Path
import random
import re
import shutil
import subprocess
import sys
from urllib.parse import unquote, urlsplit
import wave


ROOT = Path(__file__).resolve().parents[1]
SOURCE_DIR = Path(__file__).resolve().parent
DURATION = 8
FPS = 30
WIDTH = 960
HEIGHT = 540
SITE_LIMIT_BYTES = 160 * 1024 * 1024
WASM_NAME = "reactive-vcr.wasm"
PUBLIC_FILES = ("styles.css", "app.js", "engine.js", "live.js", "favicon.svg")
COMPARISON_COLUMNS = 3
TILE_WIDTH = 320
TILE_HEIGHT = 208
SAMPLER_SEGMENT_SECONDS = 4


@dataclass(frozen=True)
class Look:
    slug: str
    title: str
    description: str


def comparison_rows(looks):
    return math.ceil((len(looks) + 1) / COMPARISON_COLUMNS)


def run(args):
    process = subprocess.run(
        [str(arg) for arg in args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if process.returncode:
        command = " ".join(str(arg) for arg in args)
        raise RuntimeError(f"command failed: {command}\n{process.stderr.strip()}")
    return process.stdout


def ffmpeg(*args):
    return run(["ffmpeg", "-v", "error", "-y", *args])


def load_showcase(source_dir):
    source_dir = Path(source_dir)
    data = json.loads((source_dir / "showcase.json").read_text(encoding="utf-8"))
    looks = []
    seen = set()
    for raw in data.get("looks", []):
        look = Look(raw["slug"], raw["title"], raw["description"])
        if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", look.slug):
            raise ValueError(f"invalid showcase slug: {look.slug}")
        if look.slug in seen:
            raise ValueError(f"duplicate showcase slug: {look.slug}")
        seen.add(look.slug)
        mapping = source_dir / "maps" / f"{look.slug}.toml"
        if not mapping.is_file():
            raise ValueError(f"missing mapping for {look.slug}: {mapping.name}")
        looks.append(look)
    if not looks:
        raise ValueError("showcase must contain at least one look")
    return looks


def fill_marker(template, marker, content):
    if marker not in template:
        raise ValueError(f"template is missing {marker}")
    return template.replace(marker, content)


def render_index(template, looks):
    cards = []
    presets = []
    for look in looks:
        slug = html.escape(look.slug, quote=True)
        title = html.escape(look.title)
        description = html.escape(look.description)
        cards.append(
            f'''<article class="look-card" id="{slug}">
  <div class="look-heading"><span class="look-number">{slug[:2]}</span><h3>{title}</h3></div>
  <div class="video-frame">
    <video id="video-{slug}" controls playsinline preload="none"
      poster="posters/{slug}.jpg" data-src="media/{slug}.mp4"
      aria-label="{title} demonstration video"></video>
    <button class="load-video" type="button" data-for="video-{slug}"
      aria-controls="video-{slug}">Play {title}</button>
  </div>
  <p>{description}</p>
  <div class="card-links"><a href="#live" data-live="{slug}">Try live</a><a href="maps/{slug}.toml">View mapping</a><a href="media/{slug}.mp4" download>Download clip</a></div>
</article>'''
        )
        presets.append(
            f'''<li><button class="live-preset" type="button" data-slug="{slug}" aria-pressed="false">
  <b><span>{slug[:2]}</span> {title}</b><small>{description}</small>
</button></li>'''
        )
    template = fill_marker(template, "{{SHOWCASE_CARDS}}", "\n".join(cards))
    return fill_marker(template, "{{LIVE_PRESETS}}", "\n".join(presets))


def assemble_site(source_dir, site_dir, looks, wasm, preserve_media=False):
    source_dir = Path(source_dir)
    site_dir = Path(site_dir)
    if site_dir.exists() and not preserve_media:
        shutil.rmtree(site_dir)
    site_dir.mkdir(parents=True, exist_ok=True)
    template = (source_dir / "index.template.html").read_text(encoding="utf-8")
    (site_dir / "index.html").write_text(render_index(template, looks), encoding="utf-8")
    for filename in PUBLIC_FILES:
        shutil.copy2(source_dir / filename, site_dir / filename)
    shutil.copy2(wasm, site_dir / WASM_NAME)
    for directory in (site_dir / "maps", site_dir / "assets"):
        shutil.rmtree(directory, ignore_errors=True)
    shutil.copytree(source_dir / "maps", site_dir / "maps")
    (site_dir / "assets").mkdir()
    shutil.copy2(source_dir / "base.json", site_dir / "assets" / "base.json")
    (site_dir / "media").mkdir(exist_ok=True)
    (site_dir / "posters").mkdir(exist_ok=True)
    (site_dir / ".nojekyll").touch()


def find_font(explicit=None):
    candidates = [
        explicit,
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/Library/Fonts/Arial Bold.ttf",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return Path(candidate)
    raise FileNotFoundError(
        "no bold font found; pass --font or install fonts-dejavu-core"
    )


def synthesize_audio(path):
    sample_rate = 48_000
    rng = random.Random(4242)
    output = array.array("h")
    previous_noise = 0.0
    notes = [330, 392, 440, 523, 494, 440, 392, 294]
    for index in range(sample_rate * DURATION):
        time = index / sample_rate
        beat = time % 0.5
        eighth = time % 0.25
        step = int(time / 0.5)
        noise = rng.uniform(-1, 1)
        high = (noise - previous_noise) * 0.5
        previous_noise = noise
        kick = 0.5 * math.exp(-beat * 28) * math.sin(
            2 * math.pi * (48 * beat + 2.3 * (1 - math.exp(-beat * 35)))
        )
        bass = (
            0.19
            * math.exp(-beat * 12)
            * math.sin(2 * math.pi * [55, 55, 65.406, 49][step // 4 % 4] * time)
        )
        snare = 0.19 * noise * math.exp(-beat * 38) if step % 2 else 0
        hat = 0.055 * high * math.exp(-eighth * 145)
        note = notes[step % len(notes)]
        bell = 0.08 * math.exp(-beat * 10) * (
            math.sin(2 * math.pi * note * time)
            + 0.3 * math.sin(2 * math.pi * note * 2 * time)
        )
        fade = min(1, time / 0.015, (DURATION - time) / 0.12)
        sample = max(-0.98, min(0.98, (kick + bass + snare + hat + bell) * fade))
        output.append(int(sample * 32767))
    if sys.byteorder != "little":
        output.byteswap()
    with wave.open(str(path), "wb") as audio_file:
        audio_file.setnchannels(1)
        audio_file.setsampwidth(2)
        audio_file.setframerate(sample_rate)
        audio_file.writeframes(output.tobytes())


def create_source_assets(work_dir, font):
    work_dir.mkdir(parents=True, exist_ok=True)
    synthesize_audio(work_dir / "beat.wav")
    escaped_font = str(font).replace("'", "\\'")
    ffmpeg(
        "-f",
        "lavfi",
        "-i",
        f"testsrc2=size={WIDTH}x{HEIGHT}:rate={FPS}:duration=4",
        "-vf",
        f"drawtext=fontfile='{escaped_font}':text='REACTIVE / VCR':x=44:y=42:fontsize=54:fontcolor=white:box=1:boxcolor=black@0.72:boxborderw=14,"
        f"drawtext=fontfile='{escaped_font}':text='COLOR  /  MOTION  /  TRANSIENTS':x=46:y=116:fontsize=19:fontcolor=white:box=1:boxcolor=black@0.72:boxborderw=7",
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        "fast",
        "-crf",
        "15",
        "-pix_fmt",
        "yuv420p",
        work_dir / "calibration.mp4",
    )
    ffmpeg(
        "-f",
        "lavfi",
        "-i",
        f"mandelbrot=size={WIDTH}x{HEIGHT}:rate={FPS}:maxiter=256:start_scale=1.8:end_scale=0.08:end_pts=120:outer=outz",
        "-t",
        "4",
        "-vf",
        f"drawtext=fontfile='{escaped_font}':text='SIGNAL / DREAM':x=44:y=42:fontsize=54:fontcolor=white:box=1:boxcolor=black@0.72:boxborderw=14",
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        "fast",
        "-crf",
        "15",
        "-pix_fmt",
        "yuv420p",
        work_dir / "fractal.mp4",
    )
    (work_dir / "source-concat.txt").write_text(
        "file 'calibration.mp4'\nfile 'fractal.mp4'\n", encoding="utf-8"
    )
    ffmpeg(
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        work_dir / "source-concat.txt",
        "-c",
        "copy",
        work_dir / "source.mp4",
    )


def render_clean_reference(work_dir, site_dir):
    ffmpeg(
        "-i",
        work_dir / "source.mp4",
        "-i",
        work_dir / "beat.wav",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-shortest",
        "-movflags",
        "+faststart",
        site_dir / "media" / "00-clean-reference.mp4",
    )


def render_look(binary, work_dir, site_dir, look):
    output = site_dir / "media" / f"{look.slug}.mp4"
    run(
        [
            binary,
            "render",
            "--video",
            work_dir / "source.mp4",
            "--audio",
            work_dir / "beat.wav",
            "--map",
            site_dir / "maps" / f"{look.slug}.toml",
            "-o",
            output,
            "--audio-mode",
            "driving",
            "--quiet",
            # Quantizer 23 keeps the eleven renders within the site's size limit.
            "--quantizer",
            "23",
        ]
    )
    print(f"rendered {look.title}", flush=True)


def create_poster(video, poster):
    ffmpeg(
        "-ss",
        "1.55",
        "-i",
        video,
        "-frames:v",
        "1",
        "-update",
        "1",
        "-vf",
        "scale=480:-2",
        poster,
    )


def create_comparison(site_dir, looks, font):
    inputs = []
    filters = []
    entries = [("00-clean-reference", "Clean reference")] + [
        (look.slug, look.title) for look in looks
    ]
    escaped_font = str(font).replace("'", "\\'")
    for index, (slug, title) in enumerate(entries):
        inputs.extend(["-i", site_dir / "media" / f"{slug}.mp4"])
        safe_title = title.upper().replace("'", "\\'")
        filters.append(
            f"[{index}:v]scale={TILE_WIDTH}:180,pad={TILE_WIDTH}:{TILE_HEIGHT}:0:28:black,"
            f"drawtext=fontfile='{escaped_font}':text='{safe_title}':x=10:y=6:fontsize=15:fontcolor=white[v{index}]"
        )
    count = len(entries)
    layout = "|".join(
        f"{index % COMPARISON_COLUMNS * TILE_WIDTH}_{index // COMPARISON_COLUMNS * TILE_HEIGHT}"
        for index in range(count)
    )
    filters.append(
        "".join(f"[v{index}]" for index in range(count))
        + f"xstack=inputs={count}:layout={layout}:fill=black[grid]"
    )
    ffmpeg(
        *inputs,
        "-filter_complex",
        ";".join(filters),
        "-map",
        "[grid]",
        "-map",
        "0:a:0",
        "-c:v",
        "libx264",
        "-preset",
        "fast",
        "-crf",
        "19",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-movflags",
        "+faststart",
        "-shortest",
        site_dir / "media" / "09-comparison-grid.mp4",
    )


def create_sampler(work_dir, site_dir, looks, font):
    segments = []
    escaped_font = str(font).replace("'", "\\'")
    for index, look in enumerate(looks):
        destination = work_dir / f"reel-{index}.mp4"
        safe_title = look.title.upper().replace("'", "\\'")
        ffmpeg(
            "-i",
            site_dir / "media" / f"{look.slug}.mp4",
            "-t",
            str(SAMPLER_SEGMENT_SECONDS),
            "-vf",
            "drawbox=x=0:y=0:w=iw:h=38:color=black@0.82:t=fill,"
            f"drawtext=fontfile='{escaped_font}':text='{index + 1:02d} / {safe_title}':x=18:y=9:fontsize=20:fontcolor=white",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            destination,
        )
        segments.append(f"file 'reel-{index}.mp4'")
    (work_dir / "reel-concat.txt").write_text(
        "\n".join(segments) + "\n", encoding="utf-8"
    )
    ffmpeg(
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        work_dir / "reel-concat.txt",
        "-c",
        "copy",
        "-movflags",
        "+faststart",
        site_dir / "media" / "10-sampler-reel.mp4",
    )


def render_media(binary, work_dir, site_dir, looks, font):
    create_source_assets(work_dir, font)
    render_clean_reference(work_dir, site_dir)
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        list(
            pool.map(
                lambda look: render_look(binary, work_dir, site_dir, look), looks
            )
        )
    create_comparison(site_dir, looks, font)
    create_sampler(work_dir, site_dir, looks, font)
    slugs = ["00-clean-reference", *(look.slug for look in looks), "09-comparison-grid", "10-sampler-reel"]
    for slug in slugs:
        create_poster(
            site_dir / "media" / f"{slug}.mp4",
            site_dir / "posters" / f"{slug}.jpg",
        )


def probe_video(path):
    return json.loads(
        run(
            [
                "ffprobe",
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
                path,
            ]
        )
    )


def verify_media(site_dir, looks):
    expected = ["00-clean-reference", *(look.slug for look in looks), "09-comparison-grid", "10-sampler-reel"]
    manifest = []
    for slug in expected:
        path = site_dir / "media" / f"{slug}.mp4"
        data = probe_video(path)
        video = next(stream for stream in data["streams"] if stream["codec_type"] == "video")
        if not any(stream["codec_type"] == "audio" for stream in data["streams"]):
            raise ValueError(f"{path.name} has no audio stream")
        seconds = SAMPLER_SEGMENT_SECONDS * len(looks) if slug == "10-sampler-reel" else DURATION
        duration = float(data["format"]["duration"])
        if abs(duration - seconds) >= 0.12:
            raise ValueError(f"{path.name} duration is {duration}, expected {seconds}")
        frames = int(video["nb_frames"])
        if frames != seconds * FPS:
            raise ValueError(f"{path.name} has {frames} frames, expected {seconds * FPS}")
        expected_size = [
            WIDTH,
            TILE_HEIGHT * comparison_rows(looks) if slug == "09-comparison-grid" else HEIGHT,
        ]
        size = [video["width"], video["height"]]
        if size != expected_size:
            raise ValueError(f"{path.name} size is {size}, expected {expected_size}")
        poster = site_dir / "posters" / f"{slug}.jpg"
        if not poster.is_file():
            raise ValueError(f"missing poster: {poster.name}")
        manifest.append(
            {
                "file": f"media/{path.name}",
                "duration": duration,
                "size": size,
                "frames": frames,
                "poster": f"posters/{poster.name}",
            }
        )
    if len(list((site_dir / "media").glob("*.mp4"))) != len(expected):
        raise ValueError("site contains unexpected MP4 files")
    (site_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    return manifest


def verify_wasm(site_dir):
    path = Path(site_dir) / WASM_NAME
    if not path.is_file():
        raise ValueError(f"missing live demo engine: {WASM_NAME}")
    with path.open("rb") as wasm_file:
        if wasm_file.read(4) != b"\0asm":
            raise ValueError(f"{WASM_NAME} is not a WebAssembly module")


def verify_site(site_dir, looks, size_limit_bytes=SITE_LIMIT_BYTES):
    manifest = verify_media(site_dir, looks)
    verify_wasm(site_dir)
    verify_local_links(site_dir)
    total = enforce_size_limit(site_dir, size_limit_bytes)
    print(
        f"verified {len(manifest)} videos and {total / 1024 / 1024:.1f} MiB site",
        flush=True,
    )


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build", "site", "verify"), nargs="?", default="build")
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "demo-site")
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "release" / "reactive-vcr")
    parser.add_argument(
        "--wasm",
        type=Path,
        default=ROOT / "target" / "wasm32-unknown-unknown" / "release" / "reactive_vcr_wasm.wasm",
        help="live demo engine built from crates/reactive-vcr-wasm",
    )
    parser.add_argument("--font", type=Path)
    parser.add_argument("--size-limit-mib", type=int, default=SITE_LIMIT_BYTES // (1024 * 1024))
    return parser.parse_args()


def main():
    args = parse_args()
    looks = load_showcase(SOURCE_DIR)
    if args.command in {"build", "site"}:
        if not args.wasm.is_file():
            raise FileNotFoundError(f"live demo engine not found: {args.wasm}")
        assemble_site(
            SOURCE_DIR,
            args.output,
            looks,
            args.wasm,
            preserve_media=args.command == "site",
        )
    if args.command == "build":
        if not args.binary.is_file():
            raise FileNotFoundError(f"release binary not found: {args.binary}")
        work_dir = args.output.parent / f"{args.output.name}-build"
        if work_dir.exists():
            shutil.rmtree(work_dir)
        font = find_font(args.font)
        try:
            render_media(args.binary, work_dir, args.output, looks, font)
        finally:
            shutil.rmtree(work_dir, ignore_errors=True)
    if args.command in {"build", "verify"}:
        verify_site(args.output, looks, args.size_limit_mib * 1024 * 1024)


class _LinkParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []

    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if value and name in {"href", "src", "data-src", "poster"}:
                self.links.append(value)


def verify_local_links(site_dir):
    site_dir = Path(site_dir).resolve()
    parser = _LinkParser()
    for html_file in site_dir.rglob("*.html"):
        parser.feed(html_file.read_text(encoding="utf-8"))
        for link in parser.links:
            split = urlsplit(link)
            if split.scheme or split.netloc or link.startswith(("#", "mailto:")):
                continue
            relative = unquote(split.path)
            target = (html_file.parent / relative).resolve()
            if site_dir not in target.parents and target != site_dir:
                raise ValueError(f"local link escapes site root: {link}")
            if not target.exists():
                raise ValueError(f"missing local link target: {link}")
        parser.links.clear()


def enforce_size_limit(site_dir, limit_bytes):
    total = sum(path.stat().st_size for path in Path(site_dir).rglob("*") if path.is_file())
    if total > limit_bytes:
        raise ValueError(f"{total} bytes exceeds {limit_bytes}-byte site limit")
    return total


if __name__ == "__main__":
    main()
