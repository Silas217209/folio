#!/usr/bin/env python3
"""Validate a built bundle; structural checks also work on Linux after unzip."""
import argparse
import json
from pathlib import Path
import plistlib
import struct
import subprocess
import sys


def validate(app: Path, linkage: bool = False) -> None:
    contents = app / "Contents"
    with (contents / "Info.plist").open("rb") as stream:
        info = plistlib.load(stream)
    assert info["CFBundleIdentifier"] == "de.silas.folio", "Wrong bundle identifier"
    assert info["CFBundleExecutable"] == "folio", "Wrong executable"
    assert info["CFBundlePackageType"] == "APPL", "Not an application bundle"
    assert info["NSHighResolutionCapable"], "Retina support is missing"
    build = json.loads((contents / "Resources/build.json").read_text())
    assert info["CFBundleShortVersionString"] == build["version"], "Version mismatch"
    cpu = {"aarch64-apple-darwin": 0x0100000C,
           "x86_64-apple-darwin": 0x01000007}[build["target"]]
    for name in ("folio", "tinymist", "folio-share"):
        binary = contents / "MacOS" / name
        assert binary.stat().st_mode & 0o111, f"{name} is not executable"
        with binary.open("rb") as stream:
            magic, actual_cpu = struct.unpack("<II", stream.read(8))
        assert magic == 0xFEEDFACF and actual_cpu == cpu, f"Wrong Mach-O architecture: {name}"
        if linkage:
            dependencies = subprocess.check_output(["otool", "-L", str(binary)], text=True)
            for line in dependencies.splitlines()[1:]:
                library = line.strip().split(" (", 1)[0]
                assert library.startswith(("/System/Library/", "/usr/lib/", "@rpath/libswift")), (
                    f"{name} links to a non-system library that must be bundled: {library}")
    icon = contents / "Resources" / info["CFBundleIconFile"]
    assert icon.read_bytes().startswith(b"icns"), "Invalid ICNS icon"
    fonts = contents / "Resources/fonts/latin-modern"
    for name in ("regular", "bold", "oblique", "boldoblique"):
        font = fonts / f"lmsans10-{name}.otf"
        assert font.read_bytes().startswith(b"OTTO"), f"Missing invoice font: {name}"
    assert (fonts / "GUST-FONT-LICENSE.TXT").is_file(), "Missing font license"
    assert (contents / "Resources/ThirdParty/Tinymist-LICENSE.txt").is_file(), "Missing Tinymist license"
    assert (contents / "Resources/InvoiceLibraryStarter/templates/Rechnung.typ").is_file(), "Missing starter library"
    assert (contents / "Resources/InvoiceLibraryStarter/lib/kontakte.typ").is_file(), "Missing starter phonebook"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("--check-linkage", action="store_true", help="requires macOS otool")
    args = parser.parse_args()
    try:
        validate(args.app, args.check_linkage)
    except (AssertionError, OSError, ValueError, KeyError, struct.error) as error:
        sys.exit(f"Bundle validation failed: {error}")
    print(f"Bundle validated: {args.app}")


if __name__ == "__main__":
    main()
