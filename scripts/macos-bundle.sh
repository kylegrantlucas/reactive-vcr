#!/usr/bin/env bash
# Builds reactive-vcr.app with the official GStreamer runtime bundled inside it,
# the way ntsc-rs and bender-rs ship their macOS apps, so users don't need
# Homebrew GStreamer.
#
#   scripts/macos-bundle.sh DEST_DIR
#
# Downloads the official universal GStreamer packages (checksum-pinned) into
# target/gst-pkgs, unpacks only the components reactive-vcr uses into
# target/gstreamer-<version>, builds reactive-vcr and reactive-vcr-gui against
# them, and writes DEST_DIR/reactive-vcr.app plus DEST_DIR/GSTREAMER-NOTICE.txt.
# The CLI ships inside the app as Contents/MacOS/reactive-vcr. Nothing is
# installed system-wide, so it needs no sudo.
#
# Environment: TARGET (default aarch64-apple-darwin); GST_PKG_DIR to keep the
# downloaded packages elsewhere (default target/gst-pkgs); PKG_CONFIG to pick
# a pkg-config binary (it must not add search paths of its own).
set -euo pipefail

GST_VERSION=1.28.8
GST_RUNTIME_SHA256=d371ab04a7632713626836c2fc234e863cc98484c74c473eba99b0fbd9b3a3fe
GST_DEVEL_SHA256=e6d380bc17be7eccf13e0154567d4718fc9321da822408c7ca0cd808b3d4a3f7

# Cerbero runtime components to bundle. Compared with the installer's default
# set this drops net, capture, visualizers, analytics and the like, and adds
# effects, the GPL codecs (x264enc) and libav (avenc_aac, avenc_ffv1 and broad
# decoder coverage for decodebin).
COMPONENTS=(
  base-system-1.0 base-crypto
  gstreamer-1.0-core gstreamer-1.0-system gstreamer-1.0-playback
  gstreamer-1.0-codecs gstreamer-1.0-codecs-restricted
  gstreamer-1.0-codecs-gpl gstreamer-1.0-codecs-gpl-restricted
  gstreamer-1.0-effects gstreamer-1.0-libav
)
# Headers and pkg-config files needed to compile against the runtime.
DEVEL_COMPONENTS=(base-system-1.0-devel gstreamer-1.0-core-devel)

dest="${1:?usage: $0 DEST_DIR}"
target="${TARGET:-aarch64-apple-darwin}"
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "unsupported target $target" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
gst="$root/target/gstreamer-$GST_VERSION"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)"
pkgs="${GST_PKG_DIR:-$root/target/gst-pkgs}/$GST_VERSION"

fetch() {
  local name="$1" sha="$2" file="$pkgs/$1"
  if [ ! -f "$file" ]; then
    echo "Downloading $name"
    curl --fail --location --silent --show-error \
      -o "$file.part" "https://gstreamer.freedesktop.org/data/pkg/osx/$GST_VERSION/$name"
    mv "$file.part" "$file"
  fi
  echo "$sha  $file" | shasum -a 256 -c --status ||
    { echo "checksum mismatch for $file" >&2; exit 1; }
}

mkdir -p "$pkgs"
fetch "gstreamer-1.0-$GST_VERSION-universal.pkg" "$GST_RUNTIME_SHA256"
fetch "gstreamer-1.0-devel-$GST_VERSION-universal.pkg" "$GST_DEVEL_SHA256"

if [ ! -f "$gst/.complete" ]; then
  echo "Unpacking GStreamer $GST_VERSION components"
  rm -rf "$gst"
  mkdir -p "$gst"
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  pkgutil --expand "$pkgs/gstreamer-1.0-$GST_VERSION-universal.pkg" "$work/runtime"
  pkgutil --expand "$pkgs/gstreamer-1.0-devel-$GST_VERSION-universal.pkg" "$work/devel"
  for c in "${COMPONENTS[@]}"; do
    aa extract -i "$work/runtime/$c-$GST_VERSION-universal.pkg/Payload" -d "$gst"
  done
  for c in "${DEVEL_COMPONENTS[@]}"; do
    aa extract -i "$work/devel/$c-$GST_VERSION-universal.pkg/Payload" -d "$gst"
  done
  touch "$gst/.complete"
fi

echo "Building reactive-vcr and reactive-vcr-gui for $target"
# The .pc files are relocatable; PKG_CONFIG_LIBDIR replaces every default
# search path so nothing links against another GStreamer install. The rpath to
# the bundled framework is linked in; rewriting it afterwards with
# install_name_tool does not fit in the binaries' load-command headers.
fw_rel="Frameworks/GStreamer.framework/Versions/1.0"
(
  cd "$root"
  PKG_CONFIG_LIBDIR="$gst/lib/pkgconfig" PKG_CONFIG_PATH="" \
    RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,@executable_path/../$fw_rel/lib" \
    cargo build --release --locked --target "$target" -p reactive-vcr-cli -p reactive-vcr-gui
)

echo "Assembling reactive-vcr.app"
app="$dest/reactive-vcr.app"
fw="$app/Contents/$fw_rel"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$fw"

# Shared libraries, plugins and helper programs (gst-plugin-scanner), thinned
# to the target architecture. The official dylibs use @rpath install names and
# find each other and their plugins relative to their own location.
(cd "$gst" && find lib libexec -type f \( -name '*.dylib' -o -path 'libexec/*' \)) |
  while read -r f; do
    mkdir -p "$fw/$(dirname "$f")"
    if lipo -archs "$gst/$f" >/dev/null 2>&1 && [ "$(lipo -archs "$gst/$f")" != "$arch" ]; then
      lipo "$gst/$f" -thin "$arch" -output "$fw/$f"
    else
      cp -p "$gst/$f" "$fw/$f"
    fi
  done

for bin in reactive-vcr-gui reactive-vcr; do
  built="$root/target/$target/release/$bin"
  # Only the bundled GStreamer (@rpath) and the OS may be linked.
  if otool -L "$built" | tail -n +2 | awk '{print $1}' |
    grep -vE '^(@rpath/|/usr/lib/|/System/Library/)'; then
    echo "$bin links libraries outside the bundle and the OS" >&2
    exit 1
  fi
  # Sign before copying: inside the bundle, codesign would also try to treat
  # the plain GStreamer.framework directory as a nested bundle.
  codesign --force --sign - "$built"
  cp "$built" "$app/Contents/MacOS/$bin"
done

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>reactive-vcr</string>
  <key>CFBundleExecutable</key><string>reactive-vcr-gui</string>
  <key>CFBundleIdentifier</key><string>io.github.kylegrantlucas.reactive-vcr</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>reactive-vcr</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

notice="GStreamer $GST_VERSION
=================

reactive-vcr.app contains the official GStreamer $GST_VERSION macOS runtime from
https://gstreamer.freedesktop.org/, unmodified apart from being thinned to
$arch, in Contents/$fw_rel.

GStreamer and most of its plugins and libraries are licensed under the GNU
LGPL 2.1 or later. Some bundled components are under the GNU GPL, notably the
x264 encoder (x264enc), which reactive-vcr uses for H.264 output; FFmpeg
(gst-libav) is built under the LGPL. The libraries are dynamically linked and
can be replaced.

The complete corresponding source, including the build recipes for every
bundled library, is published by the GStreamer project:
https://gstreamer.freedesktop.org/data/pkg/src/$GST_VERSION/cerbero-$GST_VERSION.tar.xz
"
printf '%s' "$notice" >"$app/Contents/Resources/GSTREAMER-NOTICE.txt"
printf '%s' "$notice" >"$dest/GSTREAMER-NOTICE.txt"

du -sh "$app"
echo "Wrote $app"
