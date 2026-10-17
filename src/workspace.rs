use std::{cell::RefCell, fs, path::PathBuf, rc::Rc, time::Duration};

use gpui_kit::base::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Selectable, Sizable, TitleBar,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Editor, EditorState, Input, InputEvent, InputState, TabSize},
        list::ListItem,
        resizable::{h_resizable, resizable_panel},
        tab::{Tab, TabBar},
        tree::{TreeItem, TreeState, tree},
        v_flex,
    },
    *,
};

use crate::{
    CloseTab, FindReferences, FormatDocument, GoToLine, NextTab, PopOutPreview, PreviousTab,
    Recompile, RenameSymbol, Save, SignatureHelp, TogglePreview, ToggleSidebar, ToggleVim,
    ToggleWrap,
    lsp::{self, Client as LspClient},
    lsp_provider::Provider,
    page_view::{TileCache, page_canvas},
    preview::{CompiledPreview, EditLocation, PreviewEvent, PreviewLocation, PreviewService},
    preview_layout::{FitMode, PreviewLayout},
    vim,
};

pub(crate) mod commands;
mod diagnostics;
mod language;
mod modal;
mod preview_window;
mod session;
mod settings;
mod status;
mod zoom;

struct Document {
    path: String,
    uri: String,
    editor: Entity<EditorState>,
    saved: String,
    dirty: bool,
    save_error: Option<String>,
    autosave_task: Option<Task<()>>,
    vim: vim::State,
}

struct RenameTarget {
    uri: String,
    position: lsp_types::Position,
    versions: std::collections::HashMap<String, lsp::Snapshot>,
}

pub struct Workspace {
    root: PathBuf,
    entry: String,
    library_editor: bool,
    export_busy: bool,
    export_message: Option<String>,
    closing_prompt: bool,
    command_sequence: u64,
    command_error: Option<String>,
    main_window: AnyWindowHandle,
    lsp: LspClient,
    lsp_error: Option<String>,
    diagnostic_hover: Option<(usize, Rc<gpui_kit::base::input::DiagnosticEntry>)>,
    diagnostic_hover_task: Option<Task<()>>,
    settings: crate::settings::Settings,
    settings_path: Option<PathBuf>,
    settings_error: Option<String>,
    settings_open: bool,
    vim_registers: vim::Registers,
    vim_input: Entity<InputState>,
    vim_command: Option<char>,
    rename_input: Entity<InputState>,
    rename_target: Option<RenameTarget>,
    references: Option<Vec<lsp_types::Location>>,
    signature: Option<String>,
    language_task: Option<Task<()>>,
    _lsp_task: Task<()>,
    documents: Vec<Document>,
    tree: Entity<TreeState>,
    active: usize,
    tabs: Vec<usize>,
    sidebar_visible: bool,
    preview_visible: bool,
    preview_opening: bool,
    detached_preview: Option<AnyWindowHandle>,
    preview_viewport: Size<Pixels>,
    preview_fit: FitMode,
    goto_input: Entity<InputState>,
    goto_open: bool,
    goto_error: Option<String>,
    preview: Option<CompiledPreview>,
    compiling: bool,
    compile_error: Option<String>,
    save_error: Option<String>,
    edit_revision: u64,
    follow_edit: Option<u64>,
    zoom: f32,
    service: PreviewService,
    tile_cache: Rc<RefCell<TileCache>>,
    preview_scroll: ScrollHandle,
    scroll_animation: Option<Task<()>>,
    _preview_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

fn project_tree(files: &[(String, String)], prefix: &str) -> Vec<TreeItem> {
    let mut names = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for (path, _) in files {
        let Some(relative) = path.strip_prefix(prefix) else {
            continue;
        };
        let name = relative.split('/').next().unwrap_or(relative);
        if !names.insert(name.to_owned()) {
            continue;
        }
        let id = format!("{prefix}{name}");
        let item = if relative.contains('/') {
            TreeItem::new(id.clone(), name.to_owned())
                .expanded(true)
                .children(project_tree(files, &format!("{id}/")))
        } else {
            TreeItem::new(id, name.to_owned())
        };
        items.push(item);
    }
    items
}

impl Workspace {
    pub fn open(
        project: crate::project::Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = crate::settings::Settings::path();
        let (settings, error) = path.as_ref().map_or_else(
            || (crate::settings::Settings::default(), None),
            |path| match crate::settings::Settings::load(path) {
                Ok(value) => (value, None),
                Err(error) => (
                    Default::default(),
                    Some(format!("Could not load settings: {error}")),
                ),
            },
        );
        let mut workspace = Self::with_project(project, settings, path, error, window, cx);
        workspace.preview_fit = FitMode::Width;
        if workspace.entry.starts_with('_')
            || workspace.entry.starts_with("data/")
            || workspace.entry.starts_with("lib/")
        {
            workspace.preview_visible = false;
        }
        workspace.documents[0]
            .editor
            .update(cx, |state, cx| state.focus(window, cx));
        workspace
    }

    fn with_project(
        project: crate::project::Project,
        settings: crate::settings::Settings,
        settings_path: Option<PathBuf>,
        settings_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let crate::project::Project { root, entry, files } = project;
        let library_editor = root.join("templates").is_dir();
        window.set_window_title(&format!(
            "Folio — {}",
            root.file_name().unwrap_or_default().to_string_lossy()
        ));
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.confirm_close(window, cx))
                .unwrap_or(true)
        });
        let (lsp, lsp_events) = LspClient::start_entry(root.clone(), entry.clone());
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("New symbol name"));
        let vim_input = cx.new(|cx| InputState::new(window, cx));
        let goto_input = cx.new(|cx| InputState::new(window, cx).placeholder("Line[:column]"));
        let mut subscriptions = Vec::new();
        let main_window = window.window_handle();
        let weak = cx.weak_entity();
        subscriptions.push(cx.on_window_closed(move |cx, closed| {
            let _ = weak.update(cx, |this, cx| {
                if closed == main_window.window_id() {
                    if let Some(handle) = this.detached_preview.take() {
                        cx.defer(move |cx| {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                        });
                    }
                } else if this
                    .detached_preview
                    .is_some_and(|handle| handle.window_id() == closed)
                {
                    this.detached_preview = None;
                    this.preview_visible = true;
                    cx.notify();
                }
            });
        }));
        subscriptions.push(cx.subscribe_in(
            &goto_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.finish_goto(window, cx);
                }
            },
        ));
        let weak = cx.weak_entity();
        subscriptions.push(cx.intercept_keystrokes(move |event, window, cx| {
            if weak
                .update(cx, |this, cx| {
                    this.status_key(event, window, cx) || this.vim_key(event, window, cx)
                })
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        }));
        subscriptions.push(cx.subscribe_in(
            &vim_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.finish_vim_command(window, cx);
                }
            },
        ));
        let documents: Vec<Document> = files
            .iter()
            .enumerate()
            .map(|(index, (path, source))| {
                let (document, listeners) = Self::make_document(
                    index,
                    path.clone(),
                    source.clone(),
                    &root,
                    &lsp,
                    window,
                    cx,
                );
                subscriptions.extend(listeners);
                document
            })
            .collect();
        for document in &documents {
            Self::configure_document(document, &settings, window, cx);
        }
        let tree_items = project_tree(&files, "");
        let tree = cx.new(|cx| TreeState::new(cx).items(tree_items));
        tree.update(cx, |state, cx| state.set_selected_index(Some(0), cx));
        subscriptions.push(cx.observe_in(&tree, window, |this, tree, window, cx| {
            let path = tree
                .read(cx)
                .selected_item()
                .filter(|item| !item.is_folder())
                .map(|item| item.id.clone());
            if let Some(index) = path.and_then(|path| {
                this.documents
                    .iter()
                    .position(|doc| doc.path.as_str() == path.as_ref())
            }) && (index != this.active || !this.tabs.contains(&index))
            {
                this.open_document(index, window, cx);
            }
        }));
        let documents: Vec<Document> = documents;
        let files = documents
            .iter()
            .map(|doc| {
                (
                    PathBuf::from(&doc.path),
                    doc.editor.read(cx).value().to_string(),
                )
            })
            .collect();
        let (service, events) = PreviewService::start_entry(root.clone(), entry.clone(), files);
        let preview_task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match event {
                            PreviewEvent::Document(preview) => {
                                let first_preview = this.preview.is_none();
                                this.preview = Some(preview);
                                this.refresh_fit(cx);
                                if first_preview {
                                    this.preview_scroll.set_offset(Point::default());
                                }
                            }
                            PreviewEvent::Follow(location) => this.follow_location(location, cx),
                            PreviewEvent::Error(error) => {
                                this.compile_error = Some(error);
                                this.compiling = false;
                            }
                            PreviewEvent::Compiling => this.compiling = true,
                            PreviewEvent::Compiled => {
                                this.compile_error = None;
                                this.compiling = false;
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        subscriptions.push(cx.subscribe_in(
            &rename_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.finish_rename(window, cx);
                }
            },
        ));
        let lsp_task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = lsp_events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        match event {
                            lsp::Event::Ready => {
                                this.lsp_error = None;
                                for document in &this.documents {
                                    document.editor.update(cx, |state, cx| state.refresh(cx));
                                }
                            }
                            lsp::Event::Diagnostics {
                                uri,
                                version,
                                diagnostics,
                            } => {
                                if let Some(snapshot) = this.lsp.snapshot(&uri)
                                    && version.is_none_or(|version| version == snapshot.version)
                                {
                                    this.lsp.set_diagnostics(uri.clone(), diagnostics.clone());
                                    if let Some(document) =
                                        this.documents.iter().find(|document| document.uri == uri)
                                    {
                                        document.editor.update(cx, |state, cx| {
                                            let text = state.text().clone();
                                            if let Some(set) = state.diagnostics_mut() {
                                                set.reset(&text);
                                                set.extend(diagnostics.into_iter().map(
                                                    |mut diagnostic| {
                                                        diagnostic.range = lsp::editor_range(
                                                            &snapshot.text,
                                                            diagnostic.range,
                                                        );
                                                        diagnostic
                                                    },
                                                ));
                                            }
                                            cx.notify();
                                        });
                                    }
                                }
                            }
                            lsp::Event::ApplyEdit { edit, reply } => {
                                let result = this
                                    .apply_workspace_edit(edit, None, window, cx)
                                    .map_err(|error| error.to_string());
                                if let Err(error) = &result {
                                    this.lsp_error = Some(error.clone());
                                }
                                let _ = reply.try_send(result);
                            }
                            lsp::Event::Error(error) => this.lsp_error = Some(error),
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            root,
            entry,
            library_editor,
            export_busy: false,
            export_message: None,
            closing_prompt: false,
            command_sequence: 0,
            command_error: None,
            main_window,
            lsp,
            lsp_error: None,
            diagnostic_hover: None,
            diagnostic_hover_task: None,
            settings,
            settings_path,
            settings_error,
            settings_open: false,
            vim_registers: vim::Registers::default(),
            vim_input,
            vim_command: None,
            rename_input,
            rename_target: None,
            references: None,
            signature: None,
            language_task: None,
            _lsp_task: lsp_task,
            documents,
            tree,
            active: 0,
            tabs: vec![0],
            sidebar_visible: !library_editor,
            preview_visible: true,
            preview_opening: false,
            detached_preview: None,
            preview_viewport: Size::default(),
            preview_fit: FitMode::Manual,
            goto_input,
            goto_open: false,
            goto_error: None,
            preview: None,
            compiling: true,
            compile_error: None,
            save_error: None,
            edit_revision: 0,
            follow_edit: None,
            zoom: 1.,
            service,
            tile_cache: Rc::default(),
            preview_scroll: ScrollHandle::new(),
            scroll_animation: None,
            _preview_task: preview_task,
            _subscriptions: subscriptions,
        }
    }

    fn open_document(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.diagnostic_hover_task = None;
        self.diagnostic_hover = None;
        self.goto_open = false;
        self.goto_error = None;
        self.vim_command = None;
        self.active = index;
        if self.library_editor {
            self.tabs.clear();
        }
        if !self.tabs.contains(&index) {
            self.tabs.push(index);
        }
        let path: SharedString = self.documents[index].path.clone().into();
        self.tree.update(cx, |state, cx| {
            if let Some(index) = state.index_of(&path) {
                state.set_selected_index(Some(index), cx);
            }
        });
        self.documents[index]
            .editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.documents[self.active].autosave_task = None;
        self.save_document(self.active, cx);
    }

    fn save_document(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let document = &mut self.documents[index];
        let source = document.editor.read(cx).value().to_string();
        let path = self.root.join(&document.path);
        let saved = match crate::project::save_source(&path, &source, &document.saved) {
            Ok(()) => {
                self.lsp.notify(
                    "textDocument/didSave",
                    serde_json::json!({ "textDocument": { "uri": document.uri }, "text": source }),
                );
                document.saved = source;
                document.dirty = false;
                document.save_error = None;
                true
            }
            Err(error) => {
                document.save_error = Some(format!("Could not save {}: {error}", document.path));
                false
            }
        };
        self.save_error = self
            .documents
            .iter()
            .find_map(|document| document.save_error.clone());
        cx.notify();
        saved
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        if self.library_editor {
            return;
        }
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn recompile(&mut self, _: &Recompile, _: &mut Window, _: &mut Context<Self>) {
        self.service.refresh();
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(position) = self.tabs.iter().position(|&tab| tab == index) else {
            return;
        };
        if self.library_editor {
            crate::home::back(window, cx);
            return;
        }
        self.tabs.remove(position);
        if self.active == index {
            if let Some(&next) = self
                .tabs
                .get(position.min(self.tabs.len().saturating_sub(1)))
            {
                self.open_document(next, window, cx);
            } else {
                self.tree
                    .update(cx, |state, cx| state.set_selected_index(None, cx));
            }
        }
        // A closed tab keeps its buffer and undo history for reopening.
        cx.notify();
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active, window, cx);
    }

    fn set_zoom(&mut self, zoom: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if !zoom.is_finite() {
            return;
        }
        self.preview_fit = FitMode::Manual;
        self.apply_preview_zoom(zoom.clamp(0.25, 16.), anchor, cx);
    }

    fn apply_preview_zoom(
        &mut self,
        zoom: f32,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        self.scroll_animation = None;
        self.follow_edit = None;
        let viewport = self.preview_viewport_size();
        let anchor = anchor.unwrap_or_else(|| point(viewport.width / 2., viewport.height / 2.));
        let layout = self.preview_layout(self.zoom);
        let next = self.preview_layout(zoom);
        self.preview_scroll.set_offset(layout.zoom_offset(
            &next,
            self.preview_scroll.offset(),
            anchor,
            viewport,
        ));
        self.zoom = zoom;
        cx.notify();
    }

    fn preview_layout(&self, zoom: f32) -> PreviewLayout {
        let dimensions = self
            .preview
            .as_ref()
            .map(|preview| {
                preview
                    .pages
                    .iter()
                    .map(|page| (page.width(), page.height()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        PreviewLayout::new(&dimensions, zoom, self.preview_viewport_size())
    }

    fn follow_location(&mut self, location: PreviewLocation, cx: &mut Context<Self>) {
        if !self.settings.follow_source || self.follow_edit != Some(location.revision) {
            return;
        }
        let start = self.preview_scroll.offset();
        let Some(target) = self.preview_layout(self.zoom).follow_offset(
            location.page,
            location.x,
            location.y,
            start,
            self.preview_scroll.bounds().size,
        ) else {
            return;
        };
        self.scroll_animation = Some(cx.spawn(async move |this, cx| {
            for step in 1..=14 {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let progress = step as f32 / 14.;
                let eased = 1. - (1. - progress).powi(3);
                if this
                    .update(cx, |this, cx| {
                        this.preview_scroll
                            .set_offset(start + (target - start) * eased);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .on_close_window(cx.listener(|this, _, window, cx| {
                this.close_project(&crate::CloseProject, window, cx);
            }))
            .h(px(36.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .child(
                        Button::new("open-library")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .tooltip("Library")
                            .on_click(cx.listener(|_, _, window, cx| {
                                crate::home::back(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                if crate::invoice::is_library_document(&self.root, &self.entry) {
                                    std::path::Path::new(&self.entry)
                                        .file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                } else {
                                    self.root
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                },
                            ),
                    )
                    .child(div().flex_1())
                    .when(self.documents[self.active].dirty, |this| {
                        this.child(
                            Button::new("save")
                                .ghost()
                                .small()
                                .icon(IconName::Save)
                                .tooltip(crate::runtime::shortcut("Save · Ctrl+S"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save(&Save, window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("export-pdf")
                            .ghost()
                            .small()
                            .label(if self.export_busy {
                                "Exporting…"
                            } else {
                                "Export PDF"
                            })
                            .disabled(self.export_busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.export_pdf(window, cx)),
                            ),
                    )
                    .child(self.export_menu(cx))
                    .child(
                        Button::new("open-settings")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .tooltip(crate::runtime::shortcut("Settings · Ctrl+,"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(&crate::OpenSettings, window, cx)
                            })),
                    ),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lsp = self.lsp.clone();
        let root = self.root.clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(px(36.))
                    .px_3()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Files"),
            )
            .child(
                h_flex()
                    .px_3()
                    .pt_4()
                    .pb_2()
                    .gap_2()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::FolderOpen).size(px(15.)))
                    .child(
                        self.root
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    ),
            )
            .child(div().flex_1().min_h_0().px_2().child(tree(
                &self.tree,
                move |ix, entry, selected, _, cx| {
                    let diagnostics = if entry.is_folder() {
                        Vec::new()
                    } else {
                        lsp.diagnostics(&lsp::file_uri(&root.join(entry.item().id.as_ref())))
                    };
                    let severity = diagnostics::file_severity(&diagnostics);
                    let errors = diagnostics
                        .iter()
                        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
                        .count();
                    let warnings = diagnostics
                        .iter()
                        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING))
                        .count();
                    let icon = if entry.is_folder() {
                        if entry.is_expanded() {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        }
                    } else {
                        IconName::FileText
                    };
                    ListItem::new(ix)
                        .selected(selected)
                        .h(px(30.))
                        .px_2()
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .pl(px(14.) * entry.depth() as f32)
                                .child(div().w(px(12.)).when(entry.is_folder(), |this| {
                                    this.child(
                                        Icon::new(if entry.is_expanded() {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(px(12.)),
                                    )
                                }))
                                .child(Icon::new(icon).size(px(15.)).text_color(if selected {
                                    cx.theme().primary
                                } else {
                                    cx.theme().muted_foreground
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(entry.item().label.clone()),
                                )
                                .when_some(severity, |this, severity| {
                                    this.child(
                                        Button::new(SharedString::from(format!(
                                            "file-diagnostic:{}",
                                            entry.item().id
                                        )))
                                        .ghost()
                                        .xsmall()
                                        .icon(
                                            Icon::new(IconName::TriangleAlert)
                                                .text_color(diagnostics::severity_color(severity)),
                                        )
                                        .tooltip(diagnostics::summary(errors, warnings)),
                                    )
                                }),
                        )
                },
            )))
    }

    fn editor_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = &self.documents[self.active];
        v_flex()
            .size_full()
            .min_w_0()
            .bg(cx.theme().background)
            .when(!self.library_editor, |this| {
                this.child(
                    TabBar::new("source-tabs")
                        .large()
                        .h(px(36.))
                        .flex_shrink_0()
                        .max_width(px(240.))
                        .when_some(
                            self.tabs.iter().position(|&index| index == self.active),
                            |bar, index| bar.selected_index(index),
                        )
                        .on_click(cx.listener(|this, index: &usize, window, cx| {
                            if let Some(&document) = this.tabs.get(*index) {
                                this.open_document(document, window, cx);
                            }
                        }))
                        .children(self.tabs.iter().map(|&index| {
                            let document = &self.documents[index];
                            Tab::new()
                                .label(document.path.rsplit('/').next().unwrap_or(&document.path))
                                .aria_label(format!("Source tab: {}", document.path))
                                .prefix(
                                    div()
                                        .pl_2()
                                        .child(Icon::new(IconName::FileText).size(px(14.))),
                                )
                                .suffix(
                                    h_flex()
                                        .gap_1()
                                        .pr_1()
                                        .when(document.dirty, |this| {
                                            this.child(
                                                div()
                                                    .size(px(5.))
                                                    .rounded_full()
                                                    .bg(cx.theme().primary),
                                            )
                                        })
                                        .child(
                                            Button::new(("close-tab", index))
                                                .ghost()
                                                .xsmall()
                                                .icon(IconName::X)
                                                .tooltip(crate::runtime::shortcut(
                                                    "Close tab · Ctrl+W",
                                                ))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.close_tab(index, window, cx);
                                                    },
                                                )),
                                        ),
                                )
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .relative()
                    .when(!self.tabs.is_empty(), |this| {
                        this.child(
                            Editor::new(&active.editor)
                                .readonly(
                                    self.settings.vim_enabled
                                        && active.vim.mode != vim::Mode::Insert,
                                )
                                .h_full()
                                .w_full()
                                // Kit mounts floating LSP overlays beside the text
                                // element. Their empty layout must not add a flex
                                // gap that narrows the editor and rewraps lines.
                                .gap_0()
                                .bordered(false)
                                .text_size(px(f32::from(self.settings.font_size)))
                                .aria_label(format!("Source editor: {}", active.path)),
                        )
                    })
                    .when(
                        active
                            .editor
                            .read(cx)
                            .diagnostics()
                            .is_some_and(|set| !set.is_empty()),
                        |this| {
                            this.child(
                                div()
                                    .id("inline-diagnostics")
                                    .test_support()
                                    .absolute()
                                    .inset_0()
                                    .child(self.inline_diagnostics(cx)),
                            )
                        },
                    )
                    .when(
                        self.settings.vim_enabled
                            && active.vim.mode != vim::Mode::Insert
                            && !self.tabs.is_empty(),
                        |this| this.child(self.vim_caret(cx)),
                    )
                    .when(self.tabs.is_empty(), |this| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Select a file")
                    }),
            )
            .child(self.goto_panel(cx))
            .child(self.vim_panel(cx))
            .child(self.language_panels(cx))
    }

    fn preview_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = self.preview_layout(self.zoom);
        v_flex()
            .size_full()
            .min_w_0()
            .bg(cx.theme().muted)
            .child(
                h_flex()
                    .h(px(36.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .bg(cx.theme().tab_bar)
                    .border_b_1()
                    .border_color(rgb(0xd9dce0))
                    .child(self.zoom_controls(cx))
                    .when(self.compile_error.is_some(), |this| {
                        this.child(
                            Button::new("preview-warning")
                                .ghost()
                                .small()
                                .icon(Icon::new(IconName::TriangleAlert).text_color(rgb(0x946200)))
                                .tooltip("Compilation failed")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_first_error(window, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("pop-out-preview")
                            .ghost()
                            .small()
                            .icon(if self.detached_preview.is_some() {
                                IconName::PanelRight
                            } else {
                                IconName::ExternalLink
                            })
                            .tooltip(if self.detached_preview.is_some() {
                                "Return preview to editor"
                            } else {
                                "Open preview in a separate window"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.detached_preview.is_some() {
                                    this.dock_preview(window, cx);
                                } else {
                                    this.pop_out_preview(&PopOutPreview, window, cx);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id("preview-scroll")
                    .test_support()
                    .track_scroll(&self.preview_scroll)
                    .relative()
                    .child(self.measure_preview(cx))
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        this.scroll_animation = None;
                        this.follow_edit = None;
                        if event.modifiers.control {
                            let anchor = event.position - this.preview_scroll.bounds().origin;
                            let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                            this.set_zoom(this.zoom * (delta * 0.005).exp(), Some(anchor), cx);
                            cx.stop_propagation();
                        }
                        cx.notify();
                    }))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_scroll()
                    .child(
                        v_flex()
                            .w(layout.size.width)
                            .min_w_full()
                            .min_h_full()
                            .p(px(24.))
                            .gap(px(24.))
                            .items_center()
                            .when_some(self.preview.as_ref(), |this, preview| {
                                this.children(preview.pages.iter().enumerate().map(
                                    |(index, page)| {
                                        div()
                                            .id(("preview-page", index))
                                            .test_support()
                                            .flex_shrink_0()
                                            .bg(rgb(0xffffff))
                                            .shadow_md()
                                            .overflow_hidden()
                                            .child(page_canvas(
                                                page.clone(),
                                                page.width() * layout.scale,
                                                self.tile_cache.clone(),
                                            ))
                                    },
                                ))
                            })
                            .when(self.preview.is_none(), |this| {
                                this.justify_center()
                                    .gap_3()
                                    .child(
                                        Icon::new(IconName::FileText)
                                            .size(px(32.))
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(if self.compiling {
                                        "Typesetting your document…"
                                    } else {
                                        "Your document preview will appear here"
                                    })
                            }),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_mouse_move(cx.listener(Self::delay_diagnostic_hover))
            .on_action(cx.listener(|_, _: &crate::NewInvoice, window, cx| {
                crate::home::new_invoice(window, cx)
            }))
            .on_action(cx.listener(|_, _: &crate::OpenFolder, window, cx| {
                crate::home::open_folder(window, cx)
            }))
            .on_action(cx.listener(|_, _: &crate::OpenContacts, window, cx| {
                crate::home::open_contacts(window, cx)
            }))
            .on_action(cx.listener(Self::run_command))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::fit_width))
            .on_action(cx.listener(Self::fit_page))
            .on_action(cx.listener(Self::toggle_vim))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::pop_out_preview))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(Self::toggle_wrap))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::recompile))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::close_project))
            .on_action(cx.listener(Self::format_document))
            .on_action(cx.listener(Self::rename_symbol))
            .on_action(cx.listener(Self::find_references))
            .on_action(cx.listener(Self::signature_help))
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                    if std::mem::take(&mut this.goto_open)
                        | this.rename_target.take().is_some()
                        | this.references.take().is_some()
                        | this.signature.take().is_some()
                    {
                        this.documents[this.active]
                            .editor
                            .update(cx, |state, cx| state.focus(window, cx));
                        this.language_task = None;
                        cx.notify();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::ExportPdf, window, cx| this.export_pdf(window, cx)),
            )
            .on_action(cx.listener(|this, _: &crate::EmailPdf, window, cx| {
                this.export_with(crate::share::Intent::Email, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::SharePdf, window, cx| {
                this.export_with(crate::share::Intent::ChooseApp, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::PdfHistory, window, cx| {
                crate::pdf_history::open(this.root.clone(), this.entry.clone(), window, cx)
            }))
            .on_action(
                cx.listener(|_, _: &crate::ShowLibrary, window, cx| crate::home::back(window, cx)),
            )
            .child(self.toolbar(cx))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    h_resizable("editor-workspace")
                        .when(!self.library_editor, |panels| {
                            panels.child(
                                resizable_panel()
                                    .visible(self.sidebar_visible)
                                    .size(px(225.))
                                    .size_range(px(180.)..px(340.))
                                    .child(self.sidebar(cx)),
                            )
                        })
                        .child(
                            resizable_panel()
                                .size(px(610.))
                                .size_range(px(300.)..Pixels::MAX)
                                .child(self.editor_pane(cx)),
                        )
                        .child(
                            resizable_panel()
                                .visible(self.preview_visible && self.detached_preview.is_none())
                                .size_range(px(300.)..Pixels::MAX)
                                .when(
                                    self.preview_visible && self.detached_preview.is_none(),
                                    |panel| panel.child(self.preview_pane(cx)),
                                ),
                        ),
                ),
            )
            .when_some(self.export_message.as_ref(), |this, message| {
                this.child(
                    h_flex()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .text_size(px(12.))
                        .child(div().flex_1().child(message.clone()))
                        .child(
                            Button::new("dismiss-export-message")
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_message = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.save_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .p_2()
                        .text_size(px(12.))
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
            })
            .when_some(self.lsp_error.as_ref(), |this, error| {
                this.child(
                    h_flex()
                        .p_2()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(cx.theme().danger)
                        .child(div().flex_1().child(error.clone()))
                        .child(
                            Button::new("dismiss-lsp-error")
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.lsp_error = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(self.status_bar(cx))
    }
}

#[cfg(test)]
mod tests;
