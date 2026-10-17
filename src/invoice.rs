//! A library separates documents/<type>, templates, lib, and pdf/<type>.
//! Document contents are ordinary Typst. Bookkeeping only concerns filenames,
//! number reservations, and retained exports.
pub mod archive;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Days, Local, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub kind: String,
    pub number: String,
    pub title: String,
    pub filename: String,
}
#[derive(Clone)]
pub struct Record {
    pub identity: Identity,
    pub source: PathBuf,
    pub modified: SystemTime,
}
impl Record {
    pub fn entry(&self, root: &Path) -> String {
        self.source
            .strip_prefix(root)
            .unwrap_or(&self.source)
            .to_string_lossy()
            .replace('\\', "/")
    }
    pub fn stem(&self) -> String {
        self.source
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
    pub fn modified_label(&self) -> String {
        let date: DateTime<Local> = self.modified.into();
        date.format("%d %b %Y · %H:%M").to_string()
    }
}
#[derive(Serialize, Deserialize)]
struct Reservation {
    identity: Identity,
    created_at: String,
}
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("Missing parent folder")?)?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(
        &temporary,
        format!("{}\n", serde_json::to_string_pretty(value)?),
    )?;
    fs::rename(temporary, path)?;
    Ok(())
}
/// Explicit fixtures for tests; opening a library never populates it.
#[cfg(test)]
pub(crate) fn create_test_library(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("templates"))?;
    fs::create_dir(root.join("pdf"))?;
    for (name, source) in [
        (
            "Rechnung",
            include_str!("../tests/fixtures/invoices/invoice-classic.typ"),
        ),
        (
            "Angebot",
            include_str!("../tests/fixtures/invoices/invoice-minimal.typ"),
        ),
    ] {
        write_new(
            &root.join("templates").join(format!("{name}.typ")),
            source.as_bytes(),
        )?;
    }
    Ok(())
}
pub fn templates(root: &Path) -> Result<Vec<Template>> {
    let mut result = Vec::new();
    if !root.join("templates").exists() {
        return Ok(result);
    }
    for entry in fs::read_dir(root.join("templates"))? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().is_some_and(|s| s == "typ")
            && !entry.file_name().to_string_lossy().starts_with(['.', '_'])
        {
            result.push(Template {
                name: entry
                    .path()
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: entry.path(),
            });
        }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
fn types(root: &Path) -> Result<Vec<String>> {
    Ok(templates(root)?.into_iter().map(|t| t.name).collect())
}
pub fn parse_name(filename: &str, kinds: &[String]) -> Identity {
    let stem = Path::new(filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let known = kinds
        .iter()
        .filter_map(|kind| {
            stem.strip_prefix(&format!("{kind}_"))
                .map(|tail| (kind.as_str(), tail))
        })
        .max_by_key(|(kind, _)| kind.len());
    let (kind, tail) =
        known.unwrap_or_else(|| stem.split_once('_').unwrap_or(("Other", stem.as_ref())));
    let (number, title) = tail.split_once('_').unwrap_or(("", tail));
    Identity {
        kind: kind.into(),
        number: number.into(),
        title: title.into(),
        filename: Path::new(filename)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    }
}
pub fn scan(root: &Path) -> Result<Vec<Record>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let kinds = types(root)?;
    let known: std::collections::HashMap<_, _> = reservations(root)?
        .into_iter()
        .map(|r| (r.identity.filename.clone(), r.identity))
        .collect();
    let mut records = Vec::new();
    // Read legacy root documents as well. Opening never migrates files.
    let mut directories = vec![root.to_path_buf()];
    if root.join("documents").is_dir() {
        for entry in fs::read_dir(root.join("documents"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && !entry.file_name().to_string_lossy().starts_with(['.', '_'])
            {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories {
        for entry in fs::read_dir(directory).context("Cannot read the library")? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|s| s == "typ")
                && !entry.file_name().to_string_lossy().starts_with(['.', '_'])
            {
                let filename = entry.file_name().to_string_lossy().into_owned();
                let mut hints = kinds.clone();
                if let Some(kind) = entry.path().parent().and_then(|p| p.file_name()) {
                    hints.push(kind.to_string_lossy().into_owned());
                }
                records.push(Record {
                    identity: known
                        .get(&filename)
                        .cloned()
                        .unwrap_or_else(|| parse_name(&filename, &hints)),
                    source: entry.path(),
                    modified: entry.metadata()?.modified()?,
                });
            }
        }
    }
    records.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.identity.filename.cmp(&b.identity.filename))
    });
    Ok(records)
}
pub fn identify(root: &Path, entry: &str) -> Result<Identity> {
    let path = Path::new(entry);
    let filename = path
        .file_name()
        .context("Missing document filename")?
        .to_string_lossy();
    if let Some(reservation) = reservations(root)?
        .into_iter()
        .find(|r| r.identity.filename == filename)
    {
        return Ok(reservation.identity);
    }
    let mut hints = types(root)?;
    if let Some(kind) = path.parent().and_then(|p| p.file_name()) {
        hints.push(kind.to_string_lossy().into_owned());
    }
    Ok(parse_name(&filename, &hints))
}
/// Resolve a basename from older callers/history without tying document identity
/// to a physical folder. New callers use the root-relative document entry.
pub fn resolve_entry(root: &Path, entry: &str) -> Result<String> {
    ensure!(
        is_library_document(root, entry),
        "Choose a library document"
    );
    if Path::new(entry).components().count() == 1 && !root.join(entry).is_file() {
        let identity = identify(root, entry)?;
        return Ok(format!("documents/{}/{entry}", identity.kind));
    }
    Ok(entry.to_owned())
}
/// Public exports can be grouped or in the legacy flat pdf directory.
pub fn pdfs(root: &Path) -> Result<Vec<PathBuf>> {
    let folder = root.join("pdf");
    if !folder.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut directories = vec![folder.clone()];
    for entry in fs::read_dir(&folder)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            directories.push(entry.path());
        }
    }
    for directory in directories {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|s| s == "pdf")
            {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}
pub fn contacts_path(root: &Path) -> PathBuf {
    for entry in ["lib/kontakte.typ", "data/kontakte.typ", "_kontakte.typ"] {
        let path = root.join(entry);
        if path.is_file() {
            return path;
        }
    }
    root.join("lib/kontakte.typ")
}
fn reservations(root: &Path) -> Result<Vec<Reservation>> {
    let folder = root.join(".folio/numbers");
    if !folder.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|s| s == "json") {
            ensure!(
                entry.metadata()?.len() < 64 * 1024,
                "Number reservation exceeds 64 KB"
            );
            result.push(
                serde_json::from_slice(&fs::read(entry.path())?).with_context(|| {
                    format!("Cannot read number reservation {}", entry.path().display())
                })?,
            );
        }
    }
    Ok(result)
}
fn number_parts(number: &str) -> Option<(&str, &str, u128)> {
    let start = number.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (prefix, digits) = number.split_at(start);
    if digits.is_empty() {
        return None;
    }
    Some((prefix, digits, digits.parse().ok()?))
}
fn number_key(number: &str) -> String {
    number_parts(number).map_or_else(
        || number.to_owned(),
        |(prefix, _, n)| format!("{prefix}{n}"),
    )
}
fn used_numbers(root: &Path, kind: &str) -> Result<Vec<(String, String)>> {
    let mut values: Vec<_> = reservations(root)?
        .into_iter()
        .filter(|r| r.identity.kind == kind)
        .map(|r| (r.created_at, r.identity.number))
        .collect();
    values.extend(
        scan(root)?
            .into_iter()
            .filter(|r| r.identity.kind == kind && !r.identity.number.is_empty())
            .map(|r| {
                (
                    DateTime::<Utc>::from(r.modified).to_rfc3339(),
                    r.identity.number,
                )
            }),
    );
    // External PDFs also count, so removing source files cannot accidentally
    // recycle a number that has already been exported.
    let mut kinds = types(root)?;
    kinds.push(kind.to_owned());
    for path in pdfs(root)? {
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let base = name
            .rsplit_once("__v")
            .filter(|(_, suffix)| suffix.bytes().all(|c| c.is_ascii_digit()))
            .map_or(name.as_ref(), |(base, _)| base);
        let identity = parse_name(&format!("{base}.typ"), &kinds);
        if identity.kind == kind && !identity.number.is_empty() {
            values.push((String::new(), identity.number));
        }
    }
    values.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(values)
}
pub fn suggest_number(root: &Path, kind: &str) -> Result<String> {
    let used = used_numbers(root, kind)?;
    let Some((prefix, digits, _)) = used.iter().find_map(|(_, number)| number_parts(number)) else {
        return Ok("1".into());
    };
    let max = used
        .iter()
        .filter_map(|(_, number)| number_parts(number))
        .filter(|(p, _, _)| *p == prefix)
        .map(|(_, _, n)| n)
        .max()
        .unwrap_or(0);
    let next = max
        .checked_add(1)
        .context("Number sequence is exhausted; enter another number")?;
    let width = if digits.starts_with('0') {
        digits.len()
    } else {
        0
    };
    Ok(format!("{prefix}{next:0width$}"))
}
fn validate_component(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty()
            && value.len() <= 120
            && value != "."
            && value != ".."
            && !value.starts_with('.'),
        "Enter a {label} (up to 120 characters)"
    );
    ensure!(
        value
            .chars()
            .all(|c| !c.is_control() && !['/', '\\'].contains(&c)),
        "{label} cannot contain path separators or control characters"
    );
    Ok(())
}
pub fn identity(template: &Template, number: &str, title: &str) -> Result<Identity> {
    let number = number.trim();
    let title = title.trim();
    validate_component(&template.name, "type")?;
    validate_component(number, "number")?;
    validate_component(title, "title")?;
    ensure!(
        !number.contains('_'),
        "Use letters, digits, dots or hyphens in the number; underscores separate the filename fields"
    );
    let filename = format!("{}_{number}_{title}.typ", template.name);
    ensure!(filename.len() <= 240, "The filename is too long");
    Ok(Identity {
        kind: template.name.clone(),
        number: number.into(),
        title: title.into(),
        filename,
    })
}
fn escape_placeholder(value: &str) -> Result<String> {
    let quoted = serde_json::to_string(value)?;
    Ok(quoted[1..quoted.len() - 1].to_owned())
}
pub fn create(root: &Path, template: &Template, number: &str, title: &str) -> Result<PathBuf> {
    let identity = identity(template, number, title)?;
    ensure!(
        template.path.canonicalize()?.parent()
            == Some(root.join("templates").canonicalize()?.as_path()),
        "Choose a template from this library's templates folder"
    );
    ensure!(
        !used_numbers(root, &identity.kind)?
            .iter()
            .any(|(_, n)| number_key(n) == number_key(&identity.number)),
        "Number {} is already used for {}",
        identity.number,
        identity.kind
    );
    let source = fs::read_to_string(&template.path).context("Cannot read the template as UTF-8")?;
    let today = Local::now().date_naive();
    // Placeholders live inside Typst strings. Replacements are escaped and are
    // applied once, so a title containing another placeholder remains literal.
    let values = [
        ("number", identity.number.clone()),
        ("title", identity.title.clone()),
        ("type", identity.kind.clone()),
        ("date", today.to_string()),
        (
            "due",
            today.checked_add_days(Days::new(14)).unwrap().to_string(),
        ),
    ];
    let re = regex::Regex::new(r"\{\{(number|title|type|date|due)\}\}")?;
    let replacements: std::collections::HashMap<_, _> = values
        .iter()
        .map(|(k, v)| Ok((*k, escape_placeholder(v)?)))
        .collect::<Result<_>>()?;
    let source = re
        .replace_all(&source, |caps: &regex::Captures| {
            replacements[&caps[1]].clone()
        })
        .into_owned();
    let reservations = root.join(".folio/numbers");
    fs::create_dir_all(&reservations)?;
    let key = sha256(format!("{}\0{}", identity.kind, number_key(&identity.number)).as_bytes());
    let reservation = reservations.join(format!("{key}.json"));
    let path = root
        .join("documents")
        .join(&identity.kind)
        .join(&identity.filename);
    fs::create_dir_all(path.parent().context("Missing document folder")?)?;
    let reserve = Reservation {
        identity,
        created_at: Utc::now().to_rfc3339(),
    };
    write_new(&reservation, &serde_json::to_vec_pretty(&reserve)?)
        .context("This number is already reserved")?;
    if let Err(error) = write_new(&path, source.as_bytes()) {
        let _ = fs::remove_file(&reservation);
        return Err(error).context("Could not create the document; no existing file was replaced");
    }
    Ok(path)
}
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result?;
    Ok(())
}

pub fn is_library_document(root: &Path, entry: &str) -> bool {
    use std::path::Component;
    let path = Path::new(entry);
    let parts: Vec<_> = path.components().collect();
    let valid_location = match parts.as_slice() {
        [Component::Normal(_)] => true, // legacy library
        [
            Component::Normal(folder),
            Component::Normal(kind),
            Component::Normal(_),
        ] => *folder == "documents" && !kind.to_string_lossy().starts_with(['.', '_']),
        _ => false,
    };
    valid_location
        && path.extension().is_some_and(|s| s == "typ")
        && path
            .file_name()
            .is_some_and(|s| !s.to_string_lossy().starts_with(['.', '_']))
        && root.join("templates").is_dir()
}
/// Find the root when a nested library document is opened directly from disk.
pub fn root_for_document(path: &Path) -> Option<PathBuf> {
    path.ancestors().skip(1).find_map(|root| {
        let entry = path.strip_prefix(root).ok()?.to_str()?;
        is_library_document(root, entry).then(|| root.to_path_buf())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_library_types_overrides_numbering_and_template_strings() {
        let temp = tempfile::tempdir().unwrap();
        create_test_library(temp.path()).unwrap();
        let templates = templates(temp.path()).unwrap();
        let invoice = templates.iter().find(|t| t.name == "Rechnung").unwrap();
        let quote = templates.iter().find(|t| t.name == "Angebot").unwrap();
        assert_eq!(suggest_number(temp.path(), "Rechnung").unwrap(), "1");
        let path = create(temp.path(), invoice, "0042", "Design_\"{{number}}\"").unwrap();
        assert_eq!(
            path.parent(),
            Some(temp.path().join("documents/Rechnung").as_path())
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Rechnung_0042_")
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("number: \"0042\""));
        assert!(text.contains("Design_\\\"{{number}}\\\""));
        assert_eq!(suggest_number(temp.path(), "Rechnung").unwrap(), "0043");
        assert_eq!(suggest_number(temp.path(), "Angebot").unwrap(), "1");
        assert!(create(temp.path(), invoice, "42", "Different title").is_err());
        create(temp.path(), quote, "1", "Proposal").unwrap();
        create(temp.path(), invoice, "RG-99", "Another job").unwrap();
        assert_eq!(suggest_number(temp.path(), "Rechnung").unwrap(), "RG-100");
        fs::remove_file(path).unwrap();
        assert!(create(temp.path(), invoice, "0042", "Again").is_err());
        assert_eq!(scan(temp.path()).unwrap().len(), 2);
        let original = fs::read(&invoice.path).unwrap();
        scan(temp.path()).unwrap();
        super::templates(temp.path()).unwrap();
        assert_eq!(fs::read(&invoice.path).unwrap(), original);
    }
    #[test]
    fn all_root_sources_are_listed_and_template_stems_determine_types() {
        let temp = tempfile::tempdir().unwrap();
        create_test_library(temp.path()).unwrap();
        fs::write(
            temp.path().join("templates/Service_Rechnung.typ"),
            "Number {{number}}; title {{title}}",
        )
        .unwrap();
        fs::write(
            temp.path().join("Service_Rechnung_8_Test_title.typ"),
            "Text",
        )
        .unwrap();
        fs::write(temp.path().join("notes.typ"), "Notes").unwrap();
        fs::write(temp.path().join("_kontakte.typ"), "Shared contacts").unwrap();
        fs::write(temp.path().join("_rechnung.typ"), "Shared setup").unwrap();
        fs::write(temp.path().join("templates/_shared.typ"), "Shared setup").unwrap();
        assert_eq!(templates(temp.path()).unwrap().len(), 3);
        assert!(!is_library_document(temp.path(), "_kontakte.typ"));
        fs::write(temp.path().join("pdf/Rechnung_90_Archived.pdf"), "old pdf").unwrap();
        let records = scan(temp.path()).unwrap();
        assert_eq!(records.len(), 2);
        let record = records
            .iter()
            .find(|r| r.identity.kind == "Service_Rechnung")
            .unwrap();
        assert_eq!(record.identity.number, "8");
        assert_eq!(record.identity.title, "Test_title");
        assert_eq!(suggest_number(temp.path(), "Rechnung").unwrap(), "91");
    }
    #[test]
    fn grouped_documents_exclude_shared_modules_and_keep_library_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        create_test_library(root).unwrap();
        for folder in [
            "documents/Service_Rechnung",
            "data",
            "lib",
            "pdf/Service_Rechnung",
        ] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        // The grouping still identifies a type whose template has been removed.
        let entry = "documents/Service_Rechnung/Service_Rechnung_8_Test.typ";
        fs::write(root.join(entry), "#import \"/lib/kontakte.typ\": *").unwrap();
        fs::write(root.join("lib/kontakte.typ"), "Contacts").unwrap();
        fs::write(root.join("lib/rechnung.typ"), "Layout").unwrap();
        fs::write(
            root.join("pdf/Service_Rechnung/Service_Rechnung_9_Old.pdf"),
            "PDF",
        )
        .unwrap();
        let records = scan(root).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].identity.kind, "Service_Rechnung");
        assert_eq!(records[0].identity.number, "8");
        assert_eq!(records[0].entry(root), entry);
        assert_eq!(identify(root, entry).unwrap(), records[0].identity);
        assert_eq!(suggest_number(root, "Service_Rechnung").unwrap(), "10");
        assert!(!is_library_document(root, "lib/kontakte.typ"));
        assert!(!is_library_document(root, "documents/../Rechnung.typ"));
        let project = crate::project::Project::load(&root.join(entry)).unwrap();
        assert_eq!(project.root, root.canonicalize().unwrap());
        assert_eq!(project.entry, entry);
        assert_eq!(project.files.len(), 1);
        assert_eq!(contacts_path(root), root.join("lib/kontakte.typ"));
        // Prefer the new layout, but leave other libraries usable until moved.
        fs::write(root.join("data/kontakte.typ"), "Legacy contacts").unwrap();
        assert_eq!(contacts_path(root), root.join("lib/kontakte.typ"));
        fs::remove_file(root.join("lib/kontakte.typ")).unwrap();
        assert_eq!(contacts_path(root), root.join("data/kontakte.typ"));
    }
}
