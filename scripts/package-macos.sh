#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/package-macos.sh [--target TARGET] [--output DIRECTORY]

Build Folio.app and a distributable ZIP on macOS with full Xcode installed.
TARGET: aarch64-apple-darwin or x86_64-apple-darwin (default: native Rust host).
DIRECTORY defaults to dist/macos/TARGET. Existing output is never overwritten.

Optional environment:
  MACOS_SIGNING_IDENTITY  Developer ID Application certificate; default: ad-hoc
  NOTARYTOOL_PROFILE     Stored notarytool keychain profile; enables notarization
  MACOSX_DEPLOYMENT_TARGET  Minimum supported macOS; default: 13.0

No Rust, Tinymist, or font installation is needed on the destination Mac.
The invoice library remains a separate, user-selected folder.
EOF
}

target=
output=
while (($#)); do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --target|--output)
            if (($# < 2)); then echo "Missing value for $1" >&2; exit 2; fi
            if [[ $1 == --target ]]; then target=$2; else output=$2; fi
            shift 2 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done
if [[ $(uname -s) != Darwin ]]; then
    echo 'Packaging requires a Mac with full Xcode; it cannot run on Fedora.' >&2
    exit 1
fi
for tool in cargo rustc rustup python3 curl shasum tar xcrun xcodebuild iconutil sips codesign ditto lipo otool; do
    command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 1; }
done
if ! xcrun --find metal >/dev/null 2>&1; then
    echo 'Select full Xcode with xcode-select. If Metal is missing, run: xcodebuild -downloadComponent MetalToolchain' >&2
    exit 1
fi
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$project_dir"
host=$(rustc -vV | sed -n 's/^host: //p')
target=${target:-$host}
case "$target" in
    aarch64-apple-darwin) arch=arm64 ;;
    x86_64-apple-darwin) arch=x86_64 ;;
    *) echo "Unsupported macOS target: $target" >&2; exit 2 ;;
esac
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-13.0}
identity=${MACOS_SIGNING_IDENTITY:--}
profile=${NOTARYTOOL_PROFILE:-}
if [[ -n $profile && $identity == - ]]; then
    echo 'Notarization requires MACOS_SIGNING_IDENTITY, a Developer ID Application certificate.' >&2
    exit 2
fi
output=${output:-"$project_dir/dist/macos/$target"}
# Reserve the destination, avoiding accidental replacement of an existing app.
mkdir -p "$(dirname -- "$output")"
if ! mkdir "$output"; then
    echo 'Choose a new --output directory; existing output is never overwritten.' >&2
    exit 1
fi
output=$(cd "$output" && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/folio-package.XXXXXX")
trap 'rm -rf -- "$work_dir"' EXIT
cargo metadata --locked --no-deps --format-version 1 > "$work_dir/metadata.json"
target_dir=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["target_directory"])' "$work_dir/metadata.json")

rustup target add "$target"
cargo build --locked --release --target "$target" --bin folio
app="$output/Folio.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/ThirdParty"
cp "$target_dir/$target/release/folio" "$app/Contents/MacOS/folio"
xcrun swiftc -O -parse-as-library -swift-version 5 -target "$arch-apple-macos$MACOSX_DEPLOYMENT_TARGET" \
    -framework AppKit packaging/macos/Share.swift -o "$app/Contents/MacOS/folio-share"

tinymist_version=$(sed -n 's/^pub const TINYMIST_VERSION: &str = "\([^"]*\)";/\1/p' src/runtime.rs)
[[ -n $tinymist_version ]] || { echo 'Could not read the pinned Tinymist version.' >&2; exit 1; }
archive="tinymist-$target.tar.gz"
base="https://github.com/Myriad-Dreamin/tinymist/releases/download/v$tinymist_version"
curl --fail --location --retry 3 --silent --show-error "$base/$archive" -o "$work_dir/$archive"
curl --fail --location --retry 3 --silent --show-error "$base/$archive.sha256" -o "$work_dir/checksum"
(cd "$work_dir" && shasum -a 256 --check checksum)
mkdir "$work_dir/tinymist"
tar -xzf "$work_dir/$archive" --strip-components=1 -C "$work_dir/tinymist"
cp "$work_dir/tinymist/tinymist" "$app/Contents/MacOS/tinymist"
curl --fail --location --retry 3 --silent --show-error \
    "https://raw.githubusercontent.com/Myriad-Dreamin/tinymist/v$tinymist_version/LICENSE" \
    -o "$app/Contents/Resources/ThirdParty/Tinymist-LICENSE.txt"
chmod 755 "$app/Contents/MacOS/"*

# Render the actual app icon, then let Apple's tools create the ICNS sizes.
cargo run --locked --release --target "$host" --bin folio -- \
    --render-icon "$work_dir/icon.png"
mkdir "$work_dir/folio.iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$work_dir/icon.png" --out "$work_dir/folio.iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$work_dir/icon.png" --out "$work_dir/folio.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$work_dir/folio.iconset" -o "$app/Contents/Resources/folio.icns"
ditto assets/fonts "$app/Contents/Resources/fonts"
ditto templates/invoice-pro "$app/Contents/Resources/InvoiceLibraryStarter"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
python3 - "$app" "$work_dir/metadata.json" "$target" "$tinymist_version" <<'PY'
import json, os, pathlib, plistlib, sys
app = pathlib.Path(sys.argv[1])
metadata = json.loads(pathlib.Path(sys.argv[2]).read_text())
version = next(p['version'] for p in metadata['packages'] if p['name'] == 'folio')
path = app / 'Contents/Info.plist'
with path.open('rb') as stream:
    info = plistlib.load(stream)
info.update(CFBundleVersion=version, CFBundleShortVersionString=version,
            LSMinimumSystemVersion=os.environ['MACOSX_DEPLOYMENT_TARGET'])
with path.open('wb') as stream:
    plistlib.dump(info, stream)
(app / 'Contents/Resources/build.json').write_text(json.dumps({
    'version': version, 'target': sys.argv[3], 'tinymist': sys.argv[4],
}, indent=2) + '\n')
PY

# Catch architecture mismatches and accidental Homebrew/build-machine linkage.
for binary in folio tinymist folio-share; do
    # -verify_arch consumes all remaining arguments as architecture names.
    lipo "$app/Contents/MacOS/$binary" -verify_arch "$arch"
done
python3 scripts/check-macos-bundle.py "$app" --check-linkage
if [[ $target == "$host" ]]; then
    "$app/Contents/MacOS/tinymist" --version
    "$app/Contents/MacOS/folio-share" --check
fi

sign() {
    if [[ $identity == - ]]; then
        codesign --force --sign - "$1"
    else
        codesign --force --options runtime --timestamp --sign "$identity" "$1"
    fi
}
# Sign nested executables first, then the outer app. Do not use --deep to sign.
sign "$app/Contents/MacOS/tinymist"
sign "$app/Contents/MacOS/folio-share"
sign "$app"
for item in "$app/Contents/MacOS/tinymist" "$app/Contents/MacOS/folio-share" "$app"; do
    codesign --verify --strict --verbose=2 "$item"
done
if [[ $target == "$host" ]]; then
    env -i HOME="$HOME" PATH=/usr/bin:/bin "$app/Contents/MacOS/folio" --check-runtime
fi
if [[ -n $profile ]]; then
    ditto -c -k --keepParent "$app" "$work_dir/notarize.zip"
    xcrun notarytool submit "$work_dir/notarize.zip" --keychain-profile "$profile" --wait
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
fi
ditto -c -k --keepParent "$app" "$output/Folio-$target.zip"
echo "Created $app"
echo "Created $output/Folio-$target.zip"
if [[ $identity == - ]]; then
    echo 'This is an ad-hoc signed test build. Use Developer ID signing and notarization for normal distribution.'
fi
