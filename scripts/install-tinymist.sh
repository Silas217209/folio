#!/usr/bin/env bash
set -euo pipefail

# Developer convenience: download the executable Folio runs as a subprocess.
# Distributed apps bundle it; users of those apps do not run this script.
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^pub const TINYMIST_VERSION: &str = "\([^"]*\)";/\1/p' "$project_dir/src/runtime.rs")
[[ -n $version ]] || { echo 'Could not read the pinned Tinymist version.' >&2; exit 1; }
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Darwin-arm64|Darwin-aarch64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  *) echo "Build Tinymist $version for your platform and set TINYMIST to its executable; see README.md." >&2; exit 1 ;;
esac
work_dir=$(mktemp -d)
trap 'rm -rf -- "$work_dir"' EXIT
archive="tinymist-$target.tar.gz"
base="https://github.com/Myriad-Dreamin/tinymist/releases/download/v$version"
curl --fail --location --silent --show-error "$base/$archive" -o "$work_dir/$archive"
curl --fail --location --silent --show-error "$base/$archive.sha256" -o "$work_dir/checksum"
if command -v sha256sum >/dev/null; then
  (cd "$work_dir" && sha256sum --check checksum)
else
  (cd "$work_dir" && shasum -a 256 --check checksum)
fi
mkdir -p "$project_dir/.context/bin"
tar -xzf "$work_dir/$archive" --strip-components=1 -C "$project_dir/.context/bin"
echo "Installed Tinymist $version in $project_dir/.context/bin"
