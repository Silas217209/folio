# Folio

A native Typst invoice editor built with GPUI Kit, Tinymist, and a continuous
vector preview. Invoices are ordinary editable Typst files, created from your
own templates. The library shows all documents, with type filters and search.

## Development

Install the native GPUI dependencies and Rust, then run:

```sh
./scripts/install-tinymist.sh
cargo run
```

The script downloads the pinned Tinymist **0.15.10** executable into
`.context/bin/`; it is a development convenience. Folio launches Tinymist as a
separate process for its LSP, incremental preview, and PDF export. Adding a Rust
library dependency does not install that executable. The distributed macOS app
already bundles it, so its users do not run the install script.

You can build the same executable from Rust source instead:

```sh
cargo install --git https://github.com/Myriad-Dreamin/tinymist \
  --tag v0.15.10 --locked tinymist-cli
TINYMIST="$HOME/.cargo/bin/tinymist" cargo run
```

Keep the release pinned: the Reflexo client and patched Typst dependencies in
`Cargo.toml` match this Tinymist version. Tool lookup uses `TINYMIST`, then the
app's bundled tool, then `.context/bin/tinymist`, then `tinymist` on `PATH`.

## Invoice libraries

Choose a folder from the library screen, or pass it on the command line:

```sh
cargo run -- /path/to/library
```

Opening a library never creates templates, contacts, or sample invoices. Copy
`templates/` and `lib/` from [the starter library](templates/invoice-pro/README.md)
into your chosen folder manually. Edit its fictional sender, customer, and bank
details before using it.

```text
library/
  templates/
    Rechnung.typ
    Angebot.typ
  lib/
    kontakte.typ          # contacts and bank details
    rechnung.typ          # shared invoice defaults and show rules
    layout.typ            # optional custom layout
    assets/               # optional logo and signature
  documents/
    Rechnung/
      Rechnung_1_Webdesign.typ
    Angebot/
      Angebot_1_Webdesign.typ
  pdf/
    Rechnung/
      Rechnung_1_Webdesign.pdf
      Rechnung_1_Webdesign__v0002.pdf
  .folio/
    numbers/              # permanent number reservations
    exports/              # retained PDFs, manifests, and frozen source snapshots
```

**Create** chooses a template, number, and title. The template name determines the
first filename field and document type: `[Type]_[Number]_[Title].typ`. Numbers
increment independently for each type; the proposed value is editable. Existing
documents, reserved numbers, and retained PDFs prevent reuse. Only immediate
`templates/*.typ` files appear in the chooser; `_`-prefixed modules are excluded.

The editor has one document and no sidebar or tabs for invoice libraries. Edit
all wording directly in the source, including the greeting, payment text, and
closing. Shared show rules provide layout and metadata. For example:

```typst
#import "/lib/rechnung.typ": *
#show: rechnung.with(kunde: "freundlich", nummer: "42", datum: "2026-10-17")

Sehr geehrte Damen und Herren,

hiermit berechnen wir

#line-items[
  #item([Gestaltung], quantity: 2, price: 85)
]

Bitte überweisen Sie den Rechnungsbetrag von #info.total.gross.
```

The folder menu's **Contacts…** opens `lib/kontakte.typ` in the editor, without
preview. Use `kunde: kontakt("freundlich", address: "Neue Adresse")` to override
an individual field. [Template documentation](templates/invoice-pro/README.md)
covers bank details, QR codes, editable payment sections, and shared setup.

Older libraries with root-level documents, flat PDF folders, or contacts in
`data/kontakte.typ` or `_kontakte.typ` remain readable. Folio does not move their
files automatically.

## Editing and exports

Edits autosave after a short delay. The back arrow saves and returns to the
library in the same window. Save conflicts preserve external changes and report
an error. The live preview follows the changed page and retains the last valid
render when compilation fails. Errors appear in the source; zooming rerenders
vectors at the requested resolution. Preview fit modes follow container size,
and the preview can be detached into another window.

Tinymist provides completion, hover, signatures, diagnostics, definitions,
references, rename, and formatting. Settings control Vim mode, autosave, wrapping,
font size, indentation, line numbers, and source following. Standard application
shortcuts use Control on Linux and Command on macOS; Vim keeps its Control keys.

**Export PDF** freezes the source and shared files, compiles the snapshot, and
creates a new version under `pdf/<Type>/`. Earlier PDFs are never overwritten.
The history stores timestamps and SHA-256 checksums with a separate archived PDF
and source snapshot. **Email PDF…** and **Share PDF…** hand off a retained export
to the platform's composer or sharing service. Folio does not send mail itself.

Keep the entire library folder together when moving it to another computer,
including `.folio/`. Keep the library outside the app bundle.

## Packaging

The invoice app uses [the coral icon](assets/folio-invoices-coral.svg); the
[teal icon](assets/folio.svg) is reserved for the notes version. Window and macOS
bundle icons are rendered from the same embedded SVG.

On Linux, install a release build and launcher:

```sh
cargo build --release
install -Dm755 target/release/folio ~/.local/bin/folio
install -Dm644 assets/folio-invoices-coral.svg ~/.local/share/icons/hicolor/scalable/apps/folio.svg
install -Dm644 assets/folio.desktop ~/.local/share/applications/folio.desktop
```

On macOS, `./scripts/package-macos.sh` produces a self-contained `.app` and ZIP.
Build on a Mac for Apple Silicon or Intel. [Packaging instructions](packaging/macos/README.md)
cover Xcode, signing, notarization, bundled fonts, and validation.

Preferences live under `~/.config/folio/` on Linux, honoring `XDG_CONFIG_HOME`,
and `~/Library/Application Support/de.silas.folio/` on macOS.

## Code and checks

- `src/home*`, `invoice*`, and `library.rs`: library navigation, creation, and exports.
- `src/workspace/`: editor actions, settings, diagnostics, preview controls, and tests.
- `src/lsp*`, `preview*`, and `page_view.rs`: Tinymist integration and vector rendering.
- `src/runtime.rs`, `share.rs`, and `branding.rs`: bundled tools and platform integration.
- `tests/fixtures/`: test-only Typst documents; no samples are installed on startup.
- `templates/invoice-pro/`: the optional starter library.
- `packaging/` and `scripts/`: distribution resources and build tools.

Application controls dispatch registered actions or parameterized commands from
`src/workspace/commands.rs`. UI regression tests exercise these through GPUI;
there is no separate replay executable or automation feature.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- --check-runtime
```

Tests use a native font system and headless rendering; LSP, preview, and PDF tests
start the real Tinymist executable. Tests cover incremental compilation, Unicode
edits, source navigation, zoom, detached windows, Vim, autosave conflicts, library
creation, and retained export snapshots. Mac packaging includes a non-UI runtime
check, but native UI and sharing still need a Mac smoke test.
