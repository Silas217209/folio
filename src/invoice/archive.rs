//! Append-only export history. Every successful export has its own PDF, frozen
//! project sources, timestamp, and SHA-256 manifest. No previous export is replaced.
use super::*;
use std::time::UNIX_EPOCH;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportVersion {
    pub version: u32,
    pub document: Identity,
    pub exported_at: String,
    pub pdf_filename: String,
    pub pdf_sha256: String,
    pub source_sha256: String,
    pub files: Vec<SnapshotFile>,
    #[serde(skip)]
    pub pdf: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotFile {
    pub path: String,
    pub sha256: String,
}
fn history(root: &Path, filename: &str) -> PathBuf {
    root.join(".folio/exports").join(sha256(
        Path::new(filename)
            .file_name()
            .unwrap_or_default()
            .as_encoded_bytes(),
    ))
}
fn pdf_name(stem: &str, version: u32) -> String {
    if version == 1 {
        format!("{stem}.pdf")
    } else {
        format!("{stem}__v{version:04}.pdf")
    }
}
fn reserve(root: &Path, filename: &str) -> Result<(u32, PathBuf)> {
    let folder = history(root, filename);
    fs::create_dir_all(&folder)?;
    let identity = identify(root, filename)?;
    let public_folder = root.join("pdf").join(&identity.kind);
    fs::create_dir_all(&public_folder)?;
    let stem = Path::new(filename)
        .file_stem()
        .context("Missing document name")?
        .to_string_lossy();
    let mut version = 1;
    for entry in fs::read_dir(&folder)? {
        let entry = entry?;
        if let Some(n) = entry
            .file_name()
            .to_string_lossy()
            .strip_prefix('v')
            .and_then(|s| s.parse::<u32>().ok())
        {
            version = version.max(n.saturating_add(1));
        }
    }
    loop {
        ensure!(version < u32::MAX, "PDF version sequence is exhausted");
        if public_folder.join(pdf_name(&stem, version)).exists()
            || root.join("pdf").join(pdf_name(&stem, version)).exists()
        {
            version += 1;
            continue;
        }
        let directory = folder.join(format!("v{version:04}"));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok((version, directory)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => version += 1,
            Err(error) => return Err(error.into()),
        }
    }
}
pub fn export(root: &Path, entry: &str) -> Result<ExportVersion> {
    let entry = resolve_entry(root, entry)?;
    let entry = entry.as_str();
    let root = root.canonicalize()?;
    let source = root.join(entry);
    ensure!(source.is_file(), "Save the document before exporting");
    let (version, directory) = reserve(&root, entry)?;
    let result = (|| -> Result<ExportVersion> {
        let frozen = directory.join("source");
        fs::create_dir(&frozen)?;
        let mut files = Vec::new();
        snapshot(&root, &root, &frozen, 0, &mut files, &mut 0)?;
        let source_bytes = fs::read(frozen.join(entry))?;
        let compiled = directory.join("document.pdf");
        crate::project::export_pdf(&frozen, entry, &compiled)?;
        fs::File::open(&compiled)?.sync_all()?;
        let pdf_bytes = fs::read(&compiled)?;
        ensure!(
            pdf_bytes.starts_with(b"%PDF-"),
            "Compiler did not produce a PDF"
        );
        let stem = source.file_stem().unwrap().to_string_lossy();
        let filename = pdf_name(&stem, version);
        let public = root
            .join("pdf")
            .join(identify(&root, entry)?.kind)
            .join(&filename);
        let record = ExportVersion {
            version,
            document: identify(&root, entry)?,
            exported_at: Utc::now().to_rfc3339(),
            pdf_filename: filename,
            pdf_sha256: sha256(&pdf_bytes),
            source_sha256: sha256(&source_bytes),
            files,
            pdf: public.clone(),
        };
        write_new(
            &directory.join("manifest.json"),
            &serde_json::to_vec_pretty(&record)?,
        )?;
        // Same filesystem: atomic publication fails rather than replacing an
        // existing PDF, including exports made by another running process.
        // Keep the archived PDF on a separate inode from the public copy.
        // A desktop application changing the public file cannot alter the archive.
        let publication = directory.join("publication.pdf");
        fs::copy(&compiled, &publication)?;
        fs::File::open(&publication)?.sync_all()?;
        fs::hard_link(&publication, &public)
            .context("Could not publish the PDF without replacing an existing file")?;
        fs::remove_file(&publication)?;
        let mut archived_permissions = fs::metadata(&compiled)?.permissions();
        archived_permissions.set_readonly(true);
        fs::set_permissions(&compiled, archived_permissions)?;
        let mut permissions = fs::metadata(&public)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&public, permissions)?;
        Ok(record)
    })();
    if result.is_err() {
        // A published PDF is retained even if final permission bookkeeping failed.
        let manifest = directory.join("manifest.json");
        let published = fs::read(&manifest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ExportVersion>(&bytes).ok())
            .is_some_and(|v| locate_pdf(&root, &v.document.kind, &v.pdf_filename).exists());
        if !published {
            let _ = fs::remove_dir_all(&directory);
        }
    }
    result
}
fn snapshot(
    root: &Path,
    source: &Path,
    target: &Path,
    depth: usize,
    files: &mut Vec<SnapshotFile>,
    bytes: &mut u64,
) -> Result<()> {
    ensure!(depth <= 32, "Source folder nesting exceeds 32 levels");
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let kind = entry.file_type()?;
        if [".folio", ".git", ".hg", ".svn", "target", "node_modules"].contains(&name_str.as_ref())
            || (depth == 0 && name_str == "pdf")
        {
            continue;
        }
        ensure!(
            !kind.is_symlink(),
            "Cannot freeze symbolic link {}. Put its target in the library before exporting.",
            entry.path().display()
        );
        if kind.is_dir() {
            fs::create_dir(target.join(&name))?;
            snapshot(
                root,
                &entry.path(),
                &target.join(&name),
                depth + 1,
                files,
                bytes,
            )?;
        } else if kind.is_file() {
            ensure!(files.len() < 10000, "Source snapshot exceeds 10,000 files");
            ensure!(
                *bytes + entry.metadata()?.len() <= 256 * 1024 * 1024,
                "Source snapshot exceeds 256 MB"
            );
            let content = fs::read(entry.path())?;
            *bytes += content.len() as u64;
            ensure!(
                *bytes <= 256 * 1024 * 1024,
                "Source snapshot exceeds 256 MB"
            );
            write_new(&target.join(&name), &content)?;
            files.push(SnapshotFile {
                path: entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
                sha256: sha256(&content),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}
fn locate_pdf(root: &Path, kind: &str, filename: &str) -> PathBuf {
    let grouped = root.join("pdf").join(kind).join(filename);
    let legacy = root.join("pdf").join(filename);
    if grouped.exists() || !legacy.exists() {
        grouped
    } else {
        legacy
    }
}
pub fn versions(root: &Path, entry: &str) -> Result<Vec<ExportVersion>> {
    let folder = history(root, entry);
    let mut result = Vec::new();
    if folder.exists() {
        for directory in fs::read_dir(folder)? {
            let directory = directory?;
            let manifest = directory.path().join("manifest.json");
            if !directory.file_type()?.is_dir() || !manifest.is_file() {
                continue;
            }
            ensure!(
                manifest.metadata()?.len() <= 8 * 1024 * 1024,
                "Export manifest exceeds 8 MB"
            );
            let mut record: ExportVersion = serde_json::from_slice(&fs::read(&manifest)?)
                .with_context(|| format!("Cannot read {}", manifest.display()))?;
            ensure!(
                Path::new(&record.pdf_filename).components().count() == 1
                    && record.pdf_filename.ends_with(".pdf"),
                "Invalid PDF filename in export history"
            );
            validate_component(&record.document.kind, "document type")?;
            record.pdf = locate_pdf(root, &record.document.kind, &record.pdf_filename);
            // Missing/changed files stay visible; verify() reports the problem.
            result.push(record);
        }
    }
    // Include PDFs created outside Folio or before export manifests existed.
    let stem = Path::new(entry)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    for path in pdfs(root)? {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if result.iter().any(|r| r.pdf_filename == name) {
            continue;
        }
        let version = if name == format!("{stem}.pdf") {
            Some(1)
        } else {
            name.strip_prefix(&format!("{stem}__v"))
                .and_then(|s| s.strip_suffix(".pdf"))
                .and_then(|s| s.parse().ok())
        };
        if let Some(version) = version {
            let date: DateTime<Utc> = path.metadata()?.modified().unwrap_or(UNIX_EPOCH).into();
            result.push(ExportVersion {
                version,
                document: identify(root, entry)?,
                exported_at: date.to_rfc3339(),
                pdf_filename: name,
                pdf_sha256: String::new(),
                source_sha256: String::new(),
                files: Vec::new(),
                pdf: path,
            });
        }
    }
    result.sort_by_key(|a| std::cmp::Reverse(a.version));
    Ok(result)
}
pub fn verify(version: &ExportVersion) -> Result<()> {
    let bytes = fs::read(&version.pdf)
        .with_context(|| format!("Retained PDF is missing: {}", version.pdf.display()))?;
    ensure!(
        version.pdf_sha256.is_empty() || sha256(&bytes) == version.pdf_sha256,
        "Retained PDF has changed since it was exported: {}",
        version.pdf.display()
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invoice_pro_contacts_are_shared_and_frozen_with_each_export() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("templates")).unwrap();
        fs::create_dir(root.join("lib")).unwrap();
        let contacts = include_str!("../../templates/invoice-pro/lib/kontakte.typ");
        fs::write(root.join("lib/kontakte.typ"), contacts).unwrap();
        fs::write(
            root.join("lib/rechnung.typ"),
            include_str!("../../templates/invoice-pro/lib/rechnung.typ"),
        )
        .unwrap();
        fs::write(
            root.join("templates/Rechnung.typ"),
            include_str!("../../templates/invoice-pro/templates/Rechnung.typ"),
        )
        .unwrap();
        let template = templates(root).unwrap().remove(0);
        let path = create(root, &template, "1", "Contact example").unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(scan(root).unwrap().len(), 1);
        let first = export(root, filename).unwrap();
        let bytes = fs::read(&first.pdf).unwrap();
        fs::write(
            root.join("lib/kontakte.typ"),
            contacts.replace("Firma Freundlich", "Updated contact"),
        )
        .unwrap();
        let source = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            source.replace(
                "kunde: \"freundlich\"",
                "kunde: kontakt(\"freundlich\", address: \"New road 2\")",
            ),
        )
        .unwrap();
        let second = export(root, filename).unwrap();
        assert_ne!(bytes, fs::read(&second.pdf).unwrap());
        assert_eq!(
            fs::read_to_string(history(root, filename).join("v0001/source/lib/kontakte.typ"))
                .unwrap(),
            contacts
        );
        assert_eq!(fs::read(&first.pdf).unwrap(), bytes);
        fs::write(
            &path,
            source.replace("kunde: \"freundlich\"", "kunde: \"missing-key\""),
        )
        .unwrap();
        let error = export(root, filename).unwrap_err().to_string();
        assert!(
            error.contains("Unbekannter Kontakt: missing-key"),
            "{error}"
        );
        assert_eq!(versions(root, filename).unwrap().len(), 2);
    }
    #[test]
    fn exports_keep_every_pdf_and_exact_source_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        create_test_library(temp.path()).unwrap();
        let template = templates(temp.path())
            .unwrap()
            .into_iter()
            .find(|t| t.name == "Rechnung")
            .unwrap();
        let path = create(temp.path(), &template, "1", "Design").unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        let original = fs::read(&path).unwrap();
        let first = export(temp.path(), filename).unwrap();
        assert_eq!(first.version, 1);
        assert_eq!(first.pdf_filename, "Rechnung_1_Design.pdf");
        let bytes = fs::read(&first.pdf).unwrap();
        fs::write(
            &path,
            format!("{}\nSecond version", String::from_utf8_lossy(&original)),
        )
        .unwrap();
        let second = export(temp.path(), filename).unwrap();
        assert_eq!(second.version, 2);
        assert!(second.pdf_filename.ends_with("__v0002.pdf"));
        assert_eq!(fs::read(&first.pdf).unwrap(), bytes);
        verify(&first).unwrap();
        assert_eq!(
            fs::read(
                history(temp.path(), filename)
                    .join("v0001/source/documents/Rechnung")
                    .join(filename)
            )
            .unwrap(),
            original
        );
        assert_ne!(first.source_sha256, second.source_sha256);
        assert_eq!(versions(temp.path(), filename).unwrap().len(), 2);
        fs::write(&path, "#panic(\"broken\")").unwrap();
        assert!(export(temp.path(), filename).is_err());
        assert_eq!(versions(temp.path(), filename).unwrap().len(), 2);
        assert_eq!(fs::read(&first.pdf).unwrap(), bytes);
        fs::remove_file(&first.pdf).unwrap();
        fs::write(&first.pdf, "Changed outside Folio").unwrap();
        assert!(verify(&first).is_err());
        assert_eq!(
            fs::read(history(temp.path(), filename).join("v0001/document.pdf")).unwrap(),
            bytes
        );
        fs::remove_file(path).unwrap();
        assert_eq!(suggest_number(temp.path(), "Rechnung").unwrap(), "2");
    }
    #[test]
    fn concurrent_exports_and_existing_unmanaged_pdfs_are_never_replaced() {
        let temp = tempfile::tempdir().unwrap();
        create_test_library(temp.path()).unwrap();
        let template = templates(temp.path()).unwrap().remove(0);
        let path = create(temp.path(), &template, "1", "Parallel").unwrap();
        let filename = path.file_name().unwrap().to_string_lossy().into_owned();
        let preexisting = temp.path().join("pdf").join(format!(
            "{}.pdf",
            path.file_stem().unwrap().to_string_lossy()
        ));
        fs::write(&preexisting, "Old PDF").unwrap();
        std::thread::scope(|scope| {
            let a = scope.spawn(|| export(temp.path(), &filename).unwrap());
            let b = scope.spawn(|| export(temp.path(), &filename).unwrap());
            let a = a.join().unwrap();
            let b = b.join().unwrap();
            assert_ne!(a.version, b.version);
            verify(&a).unwrap();
            verify(&b).unwrap();
        });
        assert_eq!(fs::read_to_string(preexisting).unwrap(), "Old PDF");
        assert_eq!(versions(temp.path(), &filename).unwrap().len(), 3);
    }
    #[test]
    fn moving_legacy_documents_and_pdfs_preserves_history_and_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        create_test_library(root).unwrap();
        let filename = "Rechnung_7_Legacy.typ";
        fs::write(root.join(filename), "Legacy source").unwrap();
        let first = export(root, filename).unwrap();
        let bytes = fs::read(&first.pdf).unwrap();
        let legacy_pdf = root.join("pdf").join(&first.pdf_filename);
        fs::rename(&first.pdf, &legacy_pdf).unwrap();
        let old = versions(root, filename).unwrap().remove(0);
        assert_eq!(old.pdf, legacy_pdf);
        verify(&old).unwrap();

        let entry = format!("documents/Rechnung/{filename}");
        fs::create_dir_all(root.join("documents/Rechnung")).unwrap();
        fs::rename(root.join(filename), root.join(&entry)).unwrap();
        fs::rename(legacy_pdf, &first.pdf).unwrap();
        let migrated = versions(root, &entry).unwrap().remove(0);
        assert_eq!(migrated.version, 1);
        assert_eq!(migrated.pdf, first.pdf);
        assert_eq!(fs::read(&migrated.pdf).unwrap(), bytes);
        verify(&migrated).unwrap();
        assert_eq!(
            fs::read_to_string(history(root, &entry).join("v0001/source").join(filename)).unwrap(),
            "Legacy source"
        );
        fs::write(root.join(&entry), "Updated source").unwrap();
        let second = export(root, &entry).unwrap();
        assert_eq!(second.version, 2);
        assert_eq!(second.document.filename, filename);
        assert_eq!(versions(root, &entry).unwrap().len(), 2);
        assert_eq!(resolve_entry(root, filename).unwrap(), entry);
    }
}
