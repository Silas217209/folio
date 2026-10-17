//! Shared folder/document loading. Neither the editor nor preview depends on
//! whether a project was created by the invoice or notes application.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Save without truncating an existing source on failure or overwriting edits
/// made outside this editor. Temporary files stay beside the destination so
/// the final rename is atomic.
pub fn save_source(path: &Path, source: &str, expected: &str) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let path = path
        .canonicalize()
        .context("The source file no longer exists")?;
    let current = fs::read_to_string(&path)?;
    if current == source {
        return Ok(());
    }
    ensure!(
        current == expected,
        "The file changed on disk; your editor changes have been kept unsaved"
    );
    let permissions = path.metadata()?.permissions();
    ensure!(!permissions.readonly(), "The file is read-only");
    let temporary = path.with_file_name(format!(
        ".folio-save-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.set_permissions(permissions)?;
        file.write_all(source.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone)]
pub struct Project {
    pub root: PathBuf,
    pub entry: String,
    pub files: Vec<(String, String)>,
}
impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("The project no longer exists")?;
        if path.is_file()
            && let Some(root) = crate::invoice::root_for_document(&path)
        {
            return Self::document_in(&root, &path);
        }
        let root = if path.is_file() {
            path.parent().unwrap().to_path_buf()
        } else {
            path.clone()
        };
        ensure!(root.is_dir(), "Choose a project folder or a Typst file");
        let mut files = Vec::new();
        collect(&root, &root, &mut files, 0)?;
        ensure!(
            !files.is_empty(),
            "This folder has no .typ files. Create a Typst file in it, or create an invoice from the library."
        );
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let entry = if path.is_file() {
            path.file_name().unwrap().to_string_lossy().into_owned()
        } else if files.iter().any(|(name, _)| name == "main.typ") {
            "main.typ".into()
        } else {
            files[0].0.clone()
        };
        ensure!(
            files.iter().any(|(name, _)| name == &entry),
            "Choose a .typ file"
        );
        // Keep main first, and preserve the stable alphabetical order of imports.
        files.sort_by_key(|(name, _)| name != &entry);
        Ok(Self { root, entry, files })
    }
    /// Open one library document. Other invoices stay on disk;
    /// Tinymist loads imports as needed rather than opening every invoice buffer.
    pub fn document(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("The document no longer exists")?;
        let root = crate::invoice::root_for_document(&path)
            .or_else(|| path.parent().map(Path::to_path_buf))
            .context("The document has no parent folder")?;
        Self::document_in(&root, &path)
    }
    pub fn document_in(root: &Path, path: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let path = path
            .canonicalize()
            .context("The document no longer exists")?;
        ensure!(
            path.is_file() && path.extension().is_some_and(|s| s == "typ"),
            "Choose a Typst document"
        );
        ensure!(
            path.metadata()?.len() <= 4 * 1024 * 1024,
            "Source file exceeds 4 MB"
        );
        let entry = path
            .strip_prefix(&root)
            .context("Document is outside the library")?
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).context("Cannot read the document as UTF-8")?;
        Ok(Self {
            root,
            entry: entry.clone(),
            files: vec![(entry, source)],
        })
    }
}
fn collect(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String)>,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= 32, "Project folder nesting exceeds 32 levels");
    for entry in
        fs::read_dir(directory).with_context(|| format!("Cannot read {}", directory.display()))?
    {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || ["target", "node_modules"].contains(&name.as_ref())
            || kind.is_symlink()
        {
            continue;
        }
        if kind.is_dir() {
            collect(root, &entry.path(), files, depth + 1)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|s| s == "typ") {
            ensure!(
                files.len() < 512,
                "This folder contains more than 512 Typst files. Open a smaller project folder."
            );
            ensure!(
                entry.metadata()?.len() <= 4 * 1024 * 1024,
                "{} exceeds the 4 MB source limit",
                entry.path().display()
            );
            ensure!(
                files.iter().map(|(_, source)| source.len()).sum::<usize>() as u64
                    + entry.metadata()?.len()
                    <= 64 * 1024 * 1024,
                "Project sources exceed 64 MB. Open a smaller project folder."
            );
            let source = fs::read_to_string(entry.path())
                .with_context(|| format!("Cannot read {} as UTF-8", entry.path().display()))?;
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, source));
        }
    }
    Ok(())
}
/// Compile to a temporary PDF before replacing the destination, preserving any
/// previous export if compilation fails. Called on a worker thread by the UI.
pub fn export_pdf(root: &Path, entry: &str, destination: &Path) -> Result<()> {
    let temporary = destination.with_extension(format!("{}.tmp.pdf", std::process::id()));
    let output = crate::runtime::tinymist_command()
        .arg("compile")
        .arg(root.join(entry))
        .arg(&temporary)
        .arg("--root")
        .arg(root)
        .arg("--format")
        .arg("pdf")
        .current_dir(root)
        .output()
        .context("Could not start Tinymist for PDF export")?;
    if !output.status.success() {
        let _ = fs::remove_file(&temporary);
        anyhow::bail!(
            "PDF export failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    fs::rename(&temporary, destination).context("Could not save the exported PDF")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_preserve_external_edits_deleted_files_and_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("document.typ");
        fs::write(&path, "Original").unwrap();
        save_source(&path, "Saved", "Original").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Saved");
        fs::write(&path, "External edit").unwrap();
        assert!(save_source(&path, "Editor edit", "Saved").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "External edit");
        let original = path.metadata().unwrap().permissions();
        let mut permissions = original.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions.clone()).unwrap();
        assert!(save_source(&path, "Editor edit", "External edit").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "External edit");
        fs::set_permissions(&path, original).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(save_source(&path, "Editor edit", "External edit").is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    fn folders_are_discovered_without_sample_fallbacks() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("parts")).unwrap();
        fs::create_dir_all(temp.path().join(".hidden")).unwrap();
        fs::write(temp.path().join("paper.typ"), "= Real project").unwrap();
        fs::write(temp.path().join("parts/body.typ"), "Body").unwrap();
        fs::write(temp.path().join(".hidden/main.typ"), "hidden").unwrap();
        let p = Project::load(&temp.path().join("paper.typ")).unwrap();
        assert_eq!(p.entry, "paper.typ");
        assert_eq!(p.files.len(), 2);
        fs::write(temp.path().join("main.typ"), "main").unwrap();
        assert_eq!(Project::load(temp.path()).unwrap().entry, "main.typ");
        fs::write(temp.path().join("broken.typ"), [0xff]).unwrap();
        assert!(Project::load(temp.path()).is_err());
    }
}
