use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Settings {
    pub autosave: bool,
    pub vim_enabled: bool,
    pub soft_wrap: bool,
    pub font_size: u8,
    pub tab_width: u8,
    pub line_numbers: bool,
    pub follow_source: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            autosave: true,
            vim_enabled: false,
            soft_wrap: true,
            font_size: 13,
            tab_width: 2,
            line_numbers: true,
            follow_source: true,
        }
    }
}
impl Settings {
    pub fn path() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        if cfg!(target_os = "macos") {
            return std::env::var_os("HOME").map(|home| {
                PathBuf::from(home).join("Library/Application Support/de.silas.folio/settings.json")
            });
        }
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("folio/settings.json"))
    }
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let source = match fs::read(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        let value: Value = serde_json::from_slice(&source)?;
        anyhow::ensure!(value.is_object(), "Settings must be a JSON object");
        let defaults = Self::default();
        Ok(Self {
            autosave: value["autosave"].as_bool().unwrap_or(defaults.autosave),
            vim_enabled: value["vim_enabled"]
                .as_bool()
                .unwrap_or(defaults.vim_enabled),
            soft_wrap: value["soft_wrap"].as_bool().unwrap_or(defaults.soft_wrap),
            font_size: value["font_size"]
                .as_u64()
                .filter(|n| (9..=32).contains(n))
                .map_or(defaults.font_size, |n| n as u8),
            tab_width: value["tab_width"]
                .as_u64()
                .filter(|n| [2, 4, 8].contains(n))
                .map_or(defaults.tab_width, |n| n as u8),
            line_numbers: value["line_numbers"]
                .as_bool()
                .unwrap_or(defaults.line_numbers),
            follow_source: value["follow_source"]
                .as_bool()
                .unwrap_or(defaults.follow_source),
        })
    }
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let value = json!({
            "autosave": self.autosave,
            "vim_enabled": self.vim_enabled, "soft_wrap": self.soft_wrap,
            "font_size": self.font_size, "tab_width": self.tab_width,
            "line_numbers": self.line_numbers, "follow_source": self.follow_source,
        });
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(
            &temporary,
            format!("{}\n", serde_json::to_string_pretty(&value)?),
        )?;
        fs::rename(&temporary, path)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_persist_and_old_or_invalid_fields_use_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("folio/settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let settings = Settings {
            autosave: false,
            vim_enabled: true,
            soft_wrap: false,
            font_size: 18,
            tab_width: 4,
            line_numbers: false,
            follow_source: false,
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        fs::write(
            &path,
            r#"{"vim_enabled":true,"font_size":999,"tab_width":0}"#,
        )
        .unwrap();
        assert_eq!(
            Settings::load(&path).unwrap(),
            Settings {
                vim_enabled: true,
                ..Default::default()
            }
        );
        fs::write(&path, "broken json").unwrap();
        assert!(Settings::load(&path).is_err());
    }
}
