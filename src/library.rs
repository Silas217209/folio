use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    #[serde(alias = "invoice_folder")]
    pub root: PathBuf,
}
impl Default for Library {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let documents = std::env::var_os("XDG_DOCUMENTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Documents"));
        Self {
            root: documents.join("Folio/Invoices"),
        }
    }
}
impl Library {
    pub fn path() -> Option<PathBuf> {
        crate::settings::Settings::path().map(|p| p.with_file_name("library.json"))
    }
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::invoice::atomic_json(path, self)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_persists_and_old_preferences_remain_readable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.json");
        let library = Library {
            root: temp.path().join("Documents"),
        };
        library.save(&path).unwrap();
        assert_eq!(Library::load(&path).unwrap().root, library.root);
        fs::write(
            &path,
            r#"{"invoice_folder":"/old/library","recents":[],"templates":[]}"#,
        )
        .unwrap();
        assert_eq!(
            Library::load(&path).unwrap().root,
            PathBuf::from("/old/library")
        );
        fs::write(&path, "broken").unwrap();
        assert!(Library::load(&path).is_err());
    }
}
