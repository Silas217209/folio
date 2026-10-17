# macOS packaging

Build on a Mac with `scripts/package-macos.sh`. It produces `Folio.app` and a ZIP;
drag the app into Applications to install it. Apple Silicon and Intel builds are
supported separately. The macOS build, native UI, and sharing still need a Mac
smoke test; they have not been run from the Fedora development machine.

## Build on a Mac

Install Rust through rustup, Python 3, and full Xcode. Launch Xcode once to install
the macOS components, then select it:

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
# If `xcrun --find metal` fails:
xcodebuild -downloadComponent MetalToolchain

./scripts/package-macos.sh
```

The script produces `dist/macos/<target>/Folio.app` and `Folio-<target>.zip`.
It refuses to overwrite an existing output directory; use a fresh directory for
another build:

```sh
./scripts/package-macos.sh --output dist/macos/another-build
```

The native architecture is selected automatically. `--target
aarch64-apple-darwin` and `--target x86_64-apple-darwin` are supported explicitly.
The deployment target defaults to macOS 13.0; raise it with
`MACOSX_DEPLOYMENT_TARGET`. Each app contains matching Folio, Tinymist, and sharing
helper executables. Both architectures are separate downloads, not a universal
binary.

The app includes:

- The coral invoice icon, generated from `assets/folio-invoices-coral.svg`.
- Tinymist **0.15.10**, matching the pinned Reflexo protocol. The official release
  archive is verified against its published SHA-256 checksum before extraction.
- Latin Modern Sans regular, bold, oblique, and bold oblique, with its license.
  Preview, LSP, and PDF export all receive the bundled font search path.
- A native AppKit email composer/share picker helper. Folio opens a composer; it
  does not send messages automatically. Service failures after handoff appear in
  a native alert; failures to open the service return to Folio.
- A generic starter invoice library in `Contents/Resources/InvoiceLibraryStarter`.
  Copy its contents to your chosen library folder manually if needed. Opening a
  library never initializes or modifies it automatically.

The destination Mac needs no Rust, Xcode, separate Tinymist, or font installation.
Typst Universe packages are downloaded by Tinymist when first needed, so the first
compilation of an invoice-pro document requires internet access. Existing user
libraries and personal assets are never bundled. Copy the entire invoice library
to the other Mac separately, retaining `lib`, documents, templates, and
export history. Keep it outside the application bundle.

Preferences and the chosen library path are stored in
`~/Library/Application Support/de.silas.folio/`. Finder Open With handles `.typ`
files. Application shortcuts use Command; Control-Space completion and Control-Tab
navigation remain Control, as do Vim's Control commands. Command-Q saves pending
editor changes and cancels quitting if saving fails or an export is still running.

## Signing and notarization

By default, the script uses ad-hoc signatures, suitable for local testing. These
are not Developer ID signatures and do not provide normal Gatekeeper trust for a
downloaded app. For distribution, install a **Developer ID Application** certificate
in the build Mac's keychain and create a notarytool keychain profile using Apple's
account setup instructions. No credentials are stored in this repository.

```sh
MACOS_SIGNING_IDENTITY='Developer ID Application: Your Name (TEAMID)' \
NOTARYTOOL_PROFILE='folio-notary' \
./scripts/package-macos.sh --output dist/macos/signed
```

The script signs the nested tools before the outer bundle with hardened runtime,
verifies the signatures, waits for notarization, staples the ticket, and then
creates the final ZIP. A failed signing or notarization command stops packaging.
Signing credentials and certificates stay in the build Mac's keychain.

## Validation

`scripts/check-macos-bundle.py Folio.app` validates bundle structure, metadata,
architecture, executable permissions, icon, fonts, and starter files, including
on Linux after extracting a ZIP. Add `--check-linkage` on macOS to reject
dependencies outside the system library locations. Packaging also verifies each
signature. Also test a relocated app with spaces in its path and compile an
invoice using only the bundled fonts.
`Folio.app/Contents/MacOS/folio --check-runtime` checks Folio's own tool and font
discovery without opening a window. It also reports Tinymist's build information
and starts the sharing helper in its non-UI check mode. The release is pinned and
checksum-verified during packaging; reproducible Tinymist builds can omit their
release number from `--version` output.

Before handing a build to someone, open it through Finder on the destination
architecture. Check opening a library, editing, live preview, PDF export, an email
attachment, the share picker, Command shortcuts, and reopening a `.typ` file.
Launching the application and macOS service integration cannot be validated on
Fedora.

References: [Zed's macOS build setup](https://zed.dev/docs/development/macos),
[Apple bundle structures](https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html),
[Apple notarization](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
