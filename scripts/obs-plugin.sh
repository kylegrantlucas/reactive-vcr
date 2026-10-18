#!/usr/bin/env bash
# Assemble the OBS Studio plugin layout for TARGET into DEST from target/TARGET/release/.
#   macOS:   DEST/reactive-vcr.plugin (ad-hoc signed bundle)
#   Linux:   DEST/reactive-vcr/bin/64bit/reactive-vcr.so
#   Windows: DEST/reactive-vcr/bin/64bit/reactive-vcr.dll
# COPYING (and THIRD_PARTY_LICENSES.html, if present) go inside the plugin directory.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 TARGET DEST" >&2
  exit 2
fi
target="$1"
dest="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
release="${root}/target/${target}/release"

copy_licenses() {
  cp "${root}/crates/reactive-vcr-obs/COPYING" "$1/"
  if [ -f "${root}/THIRD_PARTY_LICENSES.html" ]; then
    cp "${root}/THIRD_PARTY_LICENSES.html" "$1/"
  fi
}

case "${target}" in
  *-apple-darwin)
    version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "${root}/Cargo.toml" | head -n1)"
    bundle="${dest}/reactive-vcr.plugin"
    mkdir -p "${bundle}/Contents/MacOS" "${bundle}/Contents/Resources"
    cp "${release}/libreactive_vcr_obs.dylib" "${bundle}/Contents/MacOS/reactive-vcr"
    cat > "${bundle}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>reactive-vcr</string>
  <key>CFBundleIdentifier</key>
  <string>io.github.kylegrantlucas.reactive-vcr.obs</string>
  <key>CFBundlePackageType</key>
  <string>BNDL</string>
  <key>CFBundleName</key>
  <string>reactive-vcr</string>
  <key>CFBundleVersion</key>
  <string>${version}</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
</dict>
</plist>
EOF
    copy_licenses "${bundle}/Contents/Resources"
    # Last: any later change to the bundle would invalidate the signature.
    codesign --force --sign - "${bundle}"
    ;;
  *-linux-gnu)
    mkdir -p "${dest}/reactive-vcr/bin/64bit"
    cp "${release}/libreactive_vcr_obs.so" "${dest}/reactive-vcr/bin/64bit/reactive-vcr.so"
    copy_licenses "${dest}/reactive-vcr"
    ;;
  *-windows-msvc)
    mkdir -p "${dest}/reactive-vcr/bin/64bit"
    cp "${release}/reactive_vcr_obs.dll" "${dest}/reactive-vcr/bin/64bit/reactive-vcr.dll"
    copy_licenses "${dest}/reactive-vcr"
    ;;
  *)
    echo "unsupported target: ${target}" >&2
    exit 2
    ;;
esac
