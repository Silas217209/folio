//! Parameterized application actions. UI controls and automation share these
//! handlers; validation happens before editor/platform APIs see external data.
use super::*;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq, Deserialize, schemars::JsonSchema, Action)]
#[action(namespace = folio)]
#[serde(deny_unknown_fields)]
pub struct RunCommand {
    pub command: Command,
}
impl RunCommand {
    pub fn new(command: Command) -> Self {
        Self { command }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Snapshot {},
    OpenDocument {
        index: usize,
    },
    CloseDocument {
        index: usize,
    },
    Zoom {
        percent: f32,
    },
    Configure {
        settings: SettingsPatch,
    },
    Select {
        start: usize,
        end: usize,
    },
    Replace {
        start: usize,
        end: usize,
        text: String,
    },
    Goto {
        line: u32,
        column: u32,
    },
    ConfirmRename {
        name: String,
    },
    Reference {
        index: usize,
    },
    #[serde(rename = "vim_command")]
    Vim {
        prefix: char,
        text: String,
    },
    Save {},
    TogglePreview {},
    DetachPreview {},
    DockPreview {},
    Fit {
        mode: String,
    },
    OpenSettings {},
    ToggleSidebar {},
    FocusEditor {},
    ClosePanels {},
    FirstError {},
    DismissErrors {},
}

impl Default for Command {
    fn default() -> Self {
        Self::Snapshot {}
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsPatch {
    pub autosave: Option<bool>,
    pub vim_enabled: Option<bool>,
    pub soft_wrap: Option<bool>,
    pub font_size: Option<u8>,
    pub tab_width: Option<u8>,
    pub line_numbers: Option<bool>,
    pub follow_source: Option<bool>,
}

impl Workspace {
    pub(super) fn run_command(
        &mut self,
        action: &RunCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.dispatch_command(&action.command, window, cx);
    }

    pub(crate) fn dispatch_command(
        &mut self,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        self.command_sequence += 1;
        let result = self.execute_command(command, window, cx);
        if !matches!(command, Command::Snapshot {}) {
            self.command_error = result.as_ref().err().map(|e| format!("{e:#}"));
        }
        cx.notify();
        result
    }

    fn execute_command(
        &mut self,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        ensure!(
            window.window_handle() == self.main_window
                || matches!(
                    command,
                    Command::Zoom { .. }
                        | Command::Snapshot {}
                        | Command::DismissErrors {}
                        | Command::FirstError {}
                ),
            "This command requires the main editor window"
        );
        match command {
            Command::Snapshot {} => return Ok(self.snapshot(cx)),
            Command::OpenDocument { index } | Command::CloseDocument { index } => {
                ensure!(
                    *index < self.documents.len(),
                    "Document index out of range: {index}"
                );
                if matches!(command, Command::OpenDocument { .. }) {
                    self.open_document(*index, window, cx);
                } else {
                    self.close_tab(*index, window, cx);
                }
            }
            Command::Zoom { percent } => {
                ensure!(
                    percent.is_finite() && (25.0..=1600.0).contains(percent),
                    "Zoom must be finite and between 25 and 1600 percent"
                );
                self.set_zoom(*percent / 100., None, cx);
            }
            Command::Configure { settings } => {
                ensure!(
                    settings.font_size.is_none_or(|n| (9..=32).contains(&n)),
                    "Font size must be between 9 and 32"
                );
                ensure!(
                    settings.tab_width.is_none_or(|n| [2, 4, 8].contains(&n)),
                    "Tab width must be 2, 4, or 8"
                );
                if settings
                    .vim_enabled
                    .is_some_and(|v| v != self.settings.vim_enabled)
                {
                    self.toggle_vim(&ToggleVim, window, cx);
                }
                self.update_settings(
                    |s| {
                        if let Some(v) = settings.autosave {
                            s.autosave = v;
                        }
                        if let Some(v) = settings.soft_wrap {
                            s.soft_wrap = v;
                        }
                        if let Some(v) = settings.font_size {
                            s.font_size = v;
                        }
                        if let Some(v) = settings.tab_width {
                            s.tab_width = v;
                        }
                        if let Some(v) = settings.line_numbers {
                            s.line_numbers = v;
                        }
                        if let Some(v) = settings.follow_source {
                            s.follow_source = v;
                        }
                    },
                    window,
                    cx,
                );
                if let Some(error) = &self.settings_error {
                    anyhow::bail!("{error}");
                }
            }
            Command::Select { start, end } | Command::Replace { start, end, .. } => {
                ensure!(!self.tabs.is_empty(), "No open document");
                let editor = self.documents[self.active].editor.clone();
                let text = editor.read(cx).value();
                ensure!(
                    start <= end
                        && *end <= text.len()
                        && text.is_char_boundary(*start)
                        && text.is_char_boundary(*end),
                    "Selection must be an ordered range of UTF-8 byte boundaries inside the document"
                );
                if let Command::Replace { text, .. } = command {
                    ensure!(
                        editor.read(cx).is_editable(),
                        "Editor is read-only in the current Vim mode"
                    );
                    editor.update(cx, |state, cx| {
                        state.set_selected_range(*start..*end, cx);
                        state.replace(text.clone(), window, cx);
                    });
                } else {
                    editor.update(cx, |state, cx| state.set_selected_range(*start..*end, cx));
                }
            }
            Command::Goto { line, column } => {
                ensure!(!self.tabs.is_empty(), "No open document");
                ensure!(
                    *line > 0 && *column > 0,
                    "Line and column are one-based positive integers"
                );
                self.go_to_line(&GoToLine, window, cx);
                self.goto_input.update(cx, |state, cx| {
                    state.set_value(format!("{line}:{column}"), window, cx)
                });
                self.finish_goto(window, cx);
            }
            Command::ConfirmRename { name } => {
                ensure!(self.rename_target.is_some(), "No pending rename");
                ensure!(!name.trim().is_empty(), "Rename requires a nonempty name");
                self.rename_input
                    .update(cx, |state, cx| state.set_value(name.clone(), window, cx));
                self.finish_rename(window, cx);
            }
            Command::Reference { index } => {
                let location = self
                    .references
                    .as_ref()
                    .and_then(|r| r.get(*index))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Reference index out of range: {index}"))?;
                self.navigate(location.uri.as_str(), Some(location.range), window, cx)?;
            }
            Command::Vim { prefix, text } => {
                ensure!(
                    self.settings.vim_enabled && !self.tabs.is_empty(),
                    "Vim requires an open document and Vim enabled"
                );
                ensure!(
                    [':', '/', '?'].contains(prefix),
                    "Vim prefix must be :, /, or ?"
                );
                self.vim_command = Some(*prefix);
                self.vim_input
                    .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
                self.finish_vim_command(window, cx);
            }
            Command::Save {} => {
                ensure!(!self.tabs.is_empty(), "No open document");
                self.save(&Save, window, cx);
                if let Some(error) = &self.save_error {
                    anyhow::bail!("{error}");
                }
            }
            Command::TogglePreview {} => self.toggle_preview(&TogglePreview, window, cx),
            Command::DetachPreview {} => self.pop_out_preview(&PopOutPreview, window, cx),
            Command::DockPreview {} => self.dock_preview(window, cx),
            Command::Fit { mode } => match mode.as_str() {
                "width" => self.set_fit(FitMode::Width, cx),
                "page" => self.set_fit(FitMode::Page, cx),
                _ => anyhow::bail!("Fit mode must be width or page"),
            },
            Command::OpenSettings {} => self.open_settings(&crate::OpenSettings, window, cx),
            Command::ToggleSidebar {} => self.toggle_sidebar(&ToggleSidebar, window, cx),
            Command::FocusEditor {} => {
                ensure!(!self.tabs.is_empty(), "No open document");
                self.documents[self.active]
                    .editor
                    .update(cx, |state, cx| state.focus(window, cx));
            }
            Command::ClosePanels {} => {
                use gpui_kit::component::WindowExt;
                if self.settings_open {
                    window.close_dialog(cx);
                    self.settings_open = false;
                }
                self.goto_open = false;
                self.rename_target = None;
                self.references = None;
                self.signature = None;
                self.vim_command = None;
                self.language_task = None;
            }
            Command::FirstError {} => self.show_first_error(window, cx),
            Command::DismissErrors {} => {
                self.lsp_error = None;
                self.save_error = None;
                self.settings_error = None;
            }
        }
        Ok(Value::Null)
    }

    /// Observable state, including deferred compiler/LSP failures. Dispatching an
    /// asynchronous action acknowledges the request, not successful compilation.
    pub(crate) fn snapshot(&self, cx: &App) -> Value {
        let documents: Vec<_> = self.documents.iter().map(|d| {
            let editor = d.editor.read(cx);
            let range = editor.selected_range();
            json!({"path": d.path, "text": editor.value().as_ref(), "dirty": d.dirty, "save_error": d.save_error,
                "cursor": editor.cursor(), "selection": [range.start, range.end],
                "editable": editor.is_editable(), "mode": format!("{:?}", d.vim.mode),
                "vim_message": d.vim.message, "completion_open": editor.completion_menu_state().open,
                "diagnostics": self.lsp.diagnostics(&d.uri)})
        }).collect();
        json!({"document": documents[self.active], "active": self.active, "tabs": self.tabs, "documents": documents,
            "library_editor": self.library_editor, "tabs_visible": !self.library_editor,
            "sidebar_visible": self.sidebar_visible, "preview_visible": self.preview_visible,
            "preview_detached": self.detached_preview.is_some(), "preview_opening": self.preview_opening,
            "zoom": self.zoom, "fit": format!("{:?}", self.preview_fit),
            "preview_pages": self.preview.as_ref().map_or(0, |p| p.pages.len()),
            "preview_scroll": [f32::from(self.preview_scroll.offset().x), f32::from(self.preview_scroll.offset().y)],
            "export_busy": self.export_busy, "export_message": self.export_message,
            "compiling": self.compiling, "compile_failed": self.compile_error.is_some(), "revision": self.edit_revision,
            "settings": self.settings, "settings_open": self.settings_open, "goto_open": self.goto_open,
            "rename_pending": self.rename_target.is_some(), "references": self.references,
            "signature": self.signature, "lsp_failed": self.lsp_error.is_some(), "lsp_ready": !self.lsp.capabilities().is_null(),
            "command_sequence": self.command_sequence,
            "errors": {"command": self.command_error, "save": self.save_error,
                "settings": self.settings_error, "lsp": self.lsp_error,
                "compile": self.compile_error, "goto": self.goto_error}})
    }

    #[cfg(test)]
    pub(crate) fn preview_window(&self) -> Option<AnyWindowHandle> {
        self.detached_preview
    }

    #[cfg(test)]
    pub(crate) fn check_invariants(&self, cx: &App) -> Result<()> {
        ensure!(
            self.active < self.documents.len(),
            "Active document is invalid"
        );
        let mut tabs = std::collections::HashSet::new();
        for &index in &self.tabs {
            ensure!(
                index < self.documents.len() && tabs.insert(index),
                "Invalid or duplicate tab"
            );
        }
        ensure!(
            self.tabs.is_empty() || tabs.contains(&self.active),
            "Active tab is closed"
        );
        ensure!(
            self.zoom.is_finite()
                && self.zoom > 0.
                && (self.preview_fit != FitMode::Manual || (0.25..=16.).contains(&self.zoom)),
            "Invalid zoom"
        );
        for document in &self.documents {
            let editor = document.editor.read(cx);
            let text = editor.value();
            let range = editor.selected_range();
            ensure!(
                range.start <= range.end
                    && range.end <= text.len()
                    && text.is_char_boundary(range.start)
                    && text.is_char_boundary(range.end),
                "Invalid UTF-8 selection in {}",
                document.path
            );
            ensure!(
                editor.cursor() <= text.len() && text.is_char_boundary(editor.cursor()),
                "Invalid cursor in {}",
                document.path
            );
            ensure!(
                editor.is_editable()
                    == (!self.settings.vim_enabled || document.vim.mode == vim::Mode::Insert),
                "Vim and editor editability disagree in {}",
                document.path
            );
            ensure!(
                document.dirty == (text.as_ref() != document.saved),
                "Dirty flag disagrees with source in {}",
                document.path
            );
        }
        Ok(())
    }
}
