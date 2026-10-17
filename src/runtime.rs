//! Locate bundled tools independently of Finder's working directory and PATH.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub const TINYMIST_VERSION: &str = "0.15.10";

pub fn shortcut(label: &str) -> String {
    if cfg!(target_os = "macos") {
        label
            .replace("Ctrl+", "⌘")
            .replace("Ctrl", "⌘")
            .replace("Alt+", "⌥")
    } else {
        label.to_owned()
    }
}

fn resources_for(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    if directory.file_name()? != "MacOS" || directory.parent()?.file_name()? != "Contents" {
        return None;
    }
    Some(directory.parent()?.join("Resources"))
}

fn bundled_tool(executable: &Path, name: &str) -> Option<PathBuf> {
    resources_for(executable)?;
    let path = executable.parent()?.join(name);
    path.is_file().then_some(path)
}

fn resolve_tinymist(
    override_path: Option<PathBuf>,
    executable: Option<&Path>,
    checkout: &Path,
) -> PathBuf {
    if let Some(path) = override_path.filter(|p| !p.as_os_str().is_empty()) {
        return path;
    }
    if let Some(path) = executable.and_then(|exe| bundled_tool(exe, "tinymist")) {
        return path;
    }
    let local = checkout.join(".context/bin/tinymist");
    if local.is_file() {
        local
    } else {
        "tinymist".into()
    }
}

pub fn tinymist() -> PathBuf {
    resolve_tinymist(
        std::env::var_os("TINYMIST").map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

pub fn bundled_fonts() -> Option<PathBuf> {
    let fonts = resources_for(&std::env::current_exe().ok()?)?.join("fonts");
    fonts.is_dir().then_some(fonts)
}

/// Apply the same font search path to preview, LSP, and PDF export.
pub fn tinymist_command() -> Command {
    let mut command = Command::new(tinymist());
    if let Some(fonts) = bundled_fonts() {
        // Retain any user-configured font directories as well as the bundle.
        let mut paths = vec![fonts];
        if let Some(existing) = std::env::var_os("TYPST_FONT_PATHS") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(paths) = std::env::join_paths(paths) {
            command.env("TYPST_FONT_PATHS", paths);
        }
    }
    command
}

/// A non-UI smoke check that can run without initializing a GPU.
pub fn check() -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    let output = tinymist_command()
        .arg("--version")
        .output()
        .context("Could not start the bundled Tinymist")?;
    let version = String::from_utf8_lossy(&output.stdout);
    // Official reproducible builds may omit their release number and report
    // VERGEN_IDEMPOTENT_OUTPUT. The packager pins the release URL and verifies
    // its archive checksum rather than trusting this display string.
    anyhow::ensure!(
        output.status.success(),
        "Tinymist startup failed: {version} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fonts = bundled_fonts();
    if let Some(fonts) = &fonts {
        for face in ["regular", "bold", "oblique", "boldoblique"] {
            anyhow::ensure!(
                fonts
                    .join(format!("latin-modern/lmsans10-{face}.otf"))
                    .is_file(),
                "Missing bundled invoice font: {face}"
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        anyhow::ensure!(fonts.is_some(), "Bundled invoice fonts are missing");
        let status = Command::new(share_helper()?)
            .arg("--check")
            .stdout(std::process::Stdio::null())
            .status()?;
        anyhow::ensure!(
            status.success(),
            "The native sharing helper failed its startup check"
        );
    }
    Ok(
        serde_json::json!({ "tinymist": tinymist(), "expected_release": TINYMIST_VERSION,
        "build_information": version.trim(), "fonts": fonts }),
    )
}

#[cfg(target_os = "macos")]
pub fn share_helper() -> anyhow::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    bundled_tool(&executable, "folio-share").ok_or_else(|| anyhow::anyhow!(
        "The native sharing helper is missing. Build and open Folio.app with scripts/package-macos.sh."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_tools_survive_spaces_relocation_and_a_missing_development_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(checkout.join(".context/bin")).unwrap();
        std::fs::write(checkout.join(".context/bin/tinymist"), "development").unwrap();
        let bundle = temp
            .path()
            .join("Applications/Folio Test.app/Contents/MacOS");
        std::fs::create_dir_all(&bundle).unwrap();
        let executable = bundle.join("folio");
        let tinymist = bundle.join("tinymist");
        std::fs::write(&tinymist, "bundled").unwrap();
        assert_eq!(
            resolve_tinymist(None, Some(&executable), &checkout),
            tinymist
        );
        assert_eq!(
            resolve_tinymist(
                Some("/custom/tinymist".into()),
                Some(&executable),
                &checkout
            ),
            PathBuf::from("/custom/tinymist")
        );
        std::fs::remove_dir_all(&checkout).unwrap();
        let relocated = temp.path().join("Moved.app");
        std::fs::rename(temp.path().join("Applications/Folio Test.app"), &relocated).unwrap();
        let executable = relocated.join("Contents/MacOS/folio");
        assert_eq!(
            resolve_tinymist(None, Some(&executable), &checkout),
            relocated.join("Contents/MacOS/tinymist")
        );
        assert_eq!(
            resources_for(&executable),
            Some(relocated.join("Contents/Resources"))
        );
        std::fs::remove_file(relocated.join("Contents/MacOS/tinymist")).unwrap();
        assert_eq!(
            resolve_tinymist(None, Some(&executable), &checkout),
            PathBuf::from("tinymist")
        );
        assert_eq!(resources_for(Path::new("/usr/bin/folio")), None);
    }
}
