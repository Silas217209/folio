//! A flat local document library around the shared Typst workspace.
#[cfg(test)]
mod tests;
mod ui;
use crate::{
    invoice::{self, Record, Template},
    library::Library,
    project::Project,
    share::Intent,
    workspace::Workspace,
};
use anyhow::{Context as _, Result};
use gpui_kit::{
    assets::IconName,
    base::TestSupportExt,
    component::{
        Disableable, Icon, Sizable, TitleBar, WindowExt,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    prelude::*,
    *,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
#[derive(Default)]
struct Windows {
    home: Option<(AnyWindowHandle, WeakEntity<Home>)>,
    projects: HashMap<PathBuf, AnyWindowHandle>,
}
impl Global for Windows {}
fn options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1320.), px(900.)),
            cx,
        ))),
        window_min_size: Some(size(px(900.), px(600.))),
        app_id: Some(crate::branding::APP_ID.into()),
        icon: Some(crate::branding::window_icon()),
        ..TitleBar::window_options()
    }
}
pub fn show(cx: &mut App) {
    if !cx.has_global::<Windows>() {
        cx.set_global(Windows::default());
    }
    if let Some((handle, home)) = cx.global::<Windows>().home.clone()
        && handle
            .update(cx, |_, window, cx| {
                window.activate_window();
                let _ = home.update(cx, |this, cx| this.refresh(cx));
            })
            .is_ok()
    {
        return;
    }
    match open_window(options(cx), cx, |window, cx| {
        cx.new(|cx| Home::new(window, cx))
    }) {
        Ok((handle, home)) => cx.global_mut::<Windows>().home = Some((handle, home.downgrade())),
        Err(error) => eprintln!("Could not open Folio: {error}"),
    }
}
pub fn quit(cx: &mut App) {
    if let Some((handle, home)) = cx
        .try_global::<Windows>()
        .and_then(|state| state.home.clone())
    {
        let ready = handle
            .update(cx, |_, window, cx| {
                home.update(cx, |home, cx| {
                    home.workspace.as_ref().is_none_or(|workspace| {
                        workspace.update(cx, |workspace, cx| workspace.prepare_to_leave(window, cx))
                    })
                })
                .unwrap_or(true)
            })
            .unwrap_or(true);
        if !ready {
            return;
        }
    }
    cx.quit();
}
pub fn new_invoice(window: &mut Window, cx: &mut App) {
    visit_library(window, cx, |this, window, cx| this.start_new(window, cx));
}
pub fn open_folder(window: &mut Window, cx: &mut App) {
    visit_library(window, cx, |this, window, cx| {
        this.choose_library(window, cx)
    });
}
pub fn open_contacts(window: &mut Window, cx: &mut App) {
    visit_library(window, cx, |this, window, cx| {
        let path = invoice::contacts_path(&this.library.root);
        this.open_document(&path, window, cx);
    });
}
pub fn open_path(path: PathBuf, cx: &mut App) {
    show(cx);
    if let Some((handle, home)) = cx.global::<Windows>().home.clone() {
        let _ = handle.update(cx, |_, window, cx| {
            let _ = home.update(cx, |this, cx| {
                if path.is_dir() {
                    if this.return_to_library(window, cx) {
                        this.set_library(path, cx);
                    }
                } else {
                    this.open_document(&path, window, cx);
                }
            });
        });
    }
}
/// Navigation is deferred because an editor action may currently borrow the
/// workspace owned by Home. The same native window hosts both screens.
pub fn back(window: &mut Window, cx: &mut App) {
    visit_library(window, cx, |_, _, _| {});
}
fn visit_library(
    window: &mut Window,
    cx: &mut App,
    visit: impl FnOnce(&mut Home, &mut Window, &mut Context<Home>) + 'static,
) {
    let home = cx
        .try_global::<Windows>()
        .and_then(|windows| windows.home.clone());
    if let Some((handle, home)) = home
        && handle == window.window_handle()
    {
        window.defer(cx, move |window, cx| {
            let _ = home.update(cx, |this, cx| {
                if this.return_to_library(window, cx) {
                    visit(this, window, cx);
                }
            });
        });
    }
}
fn export_counts(root: &Path, records: &[Record]) -> HashMap<String, usize> {
    let names: HashMap<_, _> = records
        .iter()
        .map(|r| (r.stem(), r.identity.filename.clone()))
        .collect();
    let mut counts = HashMap::new();
    if let Ok(files) = invoice::pdfs(root) {
        for path in files {
            if !path.is_file() || path.extension().is_none_or(|s| s != "pdf") {
                continue;
            }
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let base = if names.contains_key(stem.as_ref()) {
                stem.as_ref()
            } else {
                stem.rsplit_once("__v")
                    .filter(|(_, v)| v.bytes().all(|c| c.is_ascii_digit()))
                    .map_or(stem.as_ref(), |(base, _)| base)
            };
            if let Some(name) = names.get(base) {
                *counts.entry(name.clone()).or_insert(0) += 1;
            }
        }
    }
    counts
}
pub(crate) struct Home {
    workspace: Option<Entity<Workspace>>,
    library: Library,
    export_counts: HashMap<String, usize>,
    config_path: Option<PathBuf>,
    records: Vec<Record>,
    templates: Vec<Template>,
    filter: Option<String>,
    search: Entity<InputState>,
    focus: FocusHandle,
    create_open: bool,
    selected: Option<Template>,
    number: Entity<InputState>,
    title: Entity<InputState>,
    create_error: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    export_path: Option<PathBuf>,
    busy: Option<String>,
    _subscriptions: Vec<Subscription>,
}
impl Home {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = Library::path();
        let loaded = path
            .as_ref()
            .map(|path| Library::load(path))
            .unwrap_or_else(|| Ok(Library::default()));
        let (library, error, config) = match loaded {
            Ok(value) => (value, None, path),
            Err(error) => (
                Library::default(),
                Some(format!(
                    "Could not load library preferences: {error}. The original file has been kept."
                )),
                None,
            ),
        };
        Self::with_library(library, config, error, window, cx)
    }
    fn with_library(
        mut library: Library,
        config_path: Option<PathBuf>,
        mut error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        window.set_window_title("Folio");
        if let Ok(path) = library.root.canonicalize() {
            library.root = path;
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let number = cx.new(|cx| InputState::new(window, cx));
        let title = cx.new(|cx| InputState::new(window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let subscriptions = vec![
            cx.observe(&search, |_, _, cx| cx.notify()),
            cx.observe(&number, |_, _, cx| cx.notify()),
            cx.observe(&title, |_, _, cx| cx.notify()),
            cx.subscribe_in(&title, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) && this.create_open {
                    this.create_document(window, cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh(cx);
                }
            }),
        ];
        let records = invoice::scan(&library.root).unwrap_or_else(|problem| {
            error = Some(problem.to_string());
            Vec::new()
        });
        let templates = invoice::templates(&library.root).unwrap_or_default();
        Self {
            workspace: None,
            export_counts: export_counts(&library.root, &records),
            library,
            config_path,
            records,
            templates,
            filter: None,
            search,
            focus,
            create_open: false,
            selected: None,
            number,
            title,
            create_error: None,
            error,
            notice: None,
            export_path: None,
            busy: None,
            _subscriptions: subscriptions,
        }
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        match invoice::scan(&self.library.root) {
            Ok(records) => self.records = records,
            Err(error) => self.error = Some(error.to_string()),
        }
        match invoice::templates(&self.library.root) {
            Ok(templates) => self.templates = templates,
            Err(error) => self.error = Some(error.to_string()),
        }
        self.export_counts = export_counts(&self.library.root, &self.records);
        cx.notify();
    }
    fn set_library(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let result = (|| -> Result<()> {
            let root = root
                .canonicalize()
                .context("The library folder does not exist")?;
            anyhow::ensure!(root.is_dir(), "Choose a library folder");
            self.library.root = root;
            self.filter = None;
            self.notice = None;
            self.export_path = None;
            if let Some(path) = &self.config_path {
                self.library.save(path)?;
            }
            Ok(())
        })();
        self.error = result.err().map(|e| e.to_string());
        self.refresh(cx);
    }
    fn choose_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open library".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let response = prompt.await;
            let _ = this.update(cx, |this, cx| match response {
                Ok(Ok(Some(paths))) if !paths.is_empty() => this.set_library(paths[0].clone(), cx),
                Ok(Ok(_)) => {}
                other => {
                    this.error = Some(format!("Could not open library: {other:?}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.create_open {
            return;
        }
        self.refresh(cx);
        self.create_error = None;
        self.selected = self
            .filter
            .as_ref()
            .and_then(|kind| self.templates.iter().find(|t| &t.name == kind))
            .or_else(|| self.templates.iter().find(|t| t.name == "Rechnung"))
            .or(self.templates.first())
            .cloned();
        let Some(template) = self.selected.clone() else {
            self.error = Some(format!(
                "No templates found in {}",
                self.library.root.join("templates").display()
            ));
            cx.notify();
            return;
        };
        self.select_template(template, window, cx);
        self.title
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.create_open = true;
        let owner = cx.entity();
        window.defer(cx, move |window, cx| {
            let focus_owner = owner.clone();
            window.open_dialog(cx, move |dialog, _, cx| {
                let weak = owner.downgrade();
                let content =
                    owner.update(cx, |this, cx| this.create_content(cx).into_any_element());
                dialog
                    .title("Create document")
                    .w(px(520.))
                    .child(content)
                    .on_close(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.create_open = false;
                            cx.notify();
                        });
                    })
            });
            let title = focus_owner.read(cx).title.clone();
            title.update(cx, |state, cx| state.focus(window, cx));
        });
        cx.notify();
    }
    fn select_template(&mut self, template: Template, window: &mut Window, cx: &mut Context<Self>) {
        self.create_error = None;
        match invoice::suggest_number(&self.library.root, &template.name) {
            Ok(number) => self
                .number
                .update(cx, |state, cx| state.set_value(number, window, cx)),
            Err(error) => self.create_error = Some(error.to_string()),
        };
        self.selected = Some(template);
        cx.notify();
    }
    fn create_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> Result<PathBuf> {
            let template = self.selected.as_ref().context("Choose a template")?;
            invoice::create(
                &self.library.root,
                template,
                &self.number.read(cx).value(),
                &self.title.read(cx).value(),
            )
        })();
        match result {
            Ok(path) => {
                self.create_error = None;
                self.create_open = false;
                self.refresh(cx);
                let owner = cx.weak_entity();
                window.defer(cx, move |window, cx| {
                    window.close_dialog(cx);
                    let _ = owner.update(cx, |this, cx| this.open_document(&path, window, cx));
                });
            }
            Err(error) => {
                self.create_error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn return_to_library(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(editor) = &self.workspace
            && !editor.update(cx, |this, cx| this.prepare_to_leave(window, cx))
        {
            return false;
        }
        self.workspace = None;
        if cx.has_global::<Windows>() {
            cx.global_mut::<Windows>()
                .projects
                .retain(|_, handle| *handle != window.window_handle());
        }
        window.set_window_title("Folio");
        self.focus.focus(window, cx);
        self.refresh(cx);
        true
    }
    fn open_document(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> Result<()> {
            let canonical = path.canonicalize()?;
            if self.workspace.is_some()
                && cx
                    .try_global::<Windows>()
                    .and_then(|windows| windows.projects.get(&canonical))
                    .copied()
                    == Some(window.window_handle())
            {
                window.activate_window();
                return Ok(());
            }
            if !self.return_to_library(window, cx) {
                return Ok(());
            }
            let project = if canonical.parent() == Some(self.library.root.as_path()) {
                Project::document(&canonical)?
            } else if canonical.starts_with(&self.library.root) {
                Project::document_in(&self.library.root, &canonical)?
            } else {
                Project::load(&canonical)?
            };
            let key = project.root.join(&project.entry);
            let editor = cx.new(|cx| Workspace::open(project, window, cx));
            self.workspace = Some(editor);
            if !cx.has_global::<Windows>() {
                cx.set_global(Windows::default());
            }
            cx.global_mut::<Windows>()
                .projects
                .insert(key, window.window_handle());
            Ok(())
        })();
        self.error = result.err().map(|e| e.to_string());
        cx.notify();
    }
    fn export(
        &mut self,
        index: usize,
        intent: Intent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.records.get(index).cloned() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        if cx.has_global::<Windows>()
            && let Some(handle) = cx.global::<Windows>().projects.get(&record.source).copied()
            && handle
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    let action: Box<dyn Action> = match intent {
                        Intent::Keep => Box::new(crate::ExportPdf),
                        Intent::Email => Box::new(crate::EmailPdf),
                        Intent::ChooseApp => Box::new(crate::SharePdf),
                    };
                    window.dispatch_action(action, cx);
                })
                .is_ok()
        {
            return;
        }
        self.busy = Some(record.identity.filename.clone());
        self.error = None;
        self.notice = None;
        self.export_path = None;
        let root = self.library.root.clone();
        let entry = record.entry(&root);
        cx.spawn_in(window, async move |this, cx| {
            let export = cx
                .background_executor()
                .spawn(async move { invoice::archive::export(&root, &entry) })
                .await;
            match export {
                Ok(version) => {
                    let subject = version
                        .document
                        .filename
                        .trim_end_matches(".typ")
                        .to_owned();
                    let path = version.pdf.clone();
                    let outcome = cx
                        .background_executor()
                        .spawn(async move { crate::share::handoff(&path, intent, &subject).await })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.busy = None;
                        this.notice = Some(format!("Saved {}", version.pdf.display()));
                        this.export_path = Some(version.pdf);
                        this.error = outcome.err().map(|e| format!("PDF retained; {e}"));
                        this.refresh(cx);
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.busy = None;
                        this.error = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }
}
impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(workspace) = &self.workspace {
            return workspace.clone().into_any_element();
        }
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .bg(rgb(0xfcfdfc))
            .text_color(rgb(0x263432))
            .on_action(
                cx.listener(|this, _: &crate::NewInvoice, window, cx| this.start_new(window, cx)),
            )
            .on_action(cx.listener(|this, _: &crate::OpenFolder, window, cx| {
                this.choose_library(window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::OpenContacts, window, cx| {
                let path = invoice::contacts_path(&this.library.root);
                this.open_document(&path, window, cx);
            }))
            .child(
                TitleBar::new()
                    .h(px(36.))
                    .border_b_1()
                    .border_color(rgb(0xe5e9e7))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0x7b8581))
                            .child("Folio"),
                    ),
            )
            .child(self.header(cx))
            .when_some(self.error.as_ref(), |view, error| {
                view.child(ui::message(error, true))
            })
            .when_some(self.notice.as_ref(), |view, message| {
                view.child(ui::message(message, false).when_some(
                    self.export_path.as_ref(),
                    |view, path| {
                        let path = path.clone();
                        view.child(
                            Button::new("reveal-export")
                                .ghost()
                                .small()
                                .icon(IconName::FolderOpen)
                                .label(crate::runtime::reveal_label())
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        )
                    },
                ))
            })
            .child(self.documents(cx))
            .into_any_element()
    }
}
