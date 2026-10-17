use super::*;

impl Workspace {
    pub(super) fn make_document(
        index: usize,
        path: String,
        source: String,
        root: &std::path::Path,
        client: &LspClient,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Document, Vec<Subscription>) {
        let uri = lsp::file_uri(&root.join(&path));
        client.sync(&uri, &source);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("typst")
                .line_number(true)
                .folding(false)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .default_value(source.clone())
        });
        let provider = Provider::new(client.clone(), uri.clone());
        let workspace = cx.weak_entity();
        editor.update(cx, |state, cx| {
            state.set_highlighter_factory(
                Rc::new(|language| {
                    (language == "typst").then(|| {
                        Box::new(crate::syntax::TypstHighlighter::default())
                            as Box<dyn gpui_kit::component::input::InputHighlighter>
                    })
                }),
                cx,
            );
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(provider.clone());
            lsp.hover_provider = Some(provider.clone());
            lsp.definition_provider = Some(provider.clone());
            lsp.code_action_providers = vec![provider.clone()];
            lsp.document_color_provider = Some(provider);
            lsp.show_document = Some(Rc::new(move |params, window, cx| {
                if params.external == Some(true) {
                    cx.open_url(params.uri.as_str());
                    return true;
                }
                let params = params.clone();
                let workspace = workspace.clone();
                // Definition callbacks run while the source editor is borrowed.
                // Defer so same-file navigation can borrow that editor safely.
                window.defer(cx, move |window, cx| {
                    let _ = workspace.update(cx, |this, cx| {
                        if let Err(error) =
                            this.navigate(params.uri.as_str(), params.selection, window, cx)
                        {
                            this.lsp_error = Some(error.to_string());
                            cx.notify();
                        }
                    });
                });
                true
            }));
        });
        let changes = cx.subscribe_in(
            &editor,
            window,
            move |this, editor, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let document = &this.documents[index];
                    let state = editor.read(cx);
                    let source = state.value().to_string();
                    this.lsp.sync(&document.uri, &source);
                    let path = document.path.clone();
                    let position = state.cursor_position();
                    let cursor = state.cursor();
                    if this.settings.vim_enabled {
                        this.documents[index].vim.observe(&source, cursor);
                    }
                    this.documents[index].dirty = source != this.documents[index].saved;
                    this.edit_revision += 1;
                    this.diagnostic_hover_task = None;
                    this.diagnostic_hover = None;
                    this.follow_edit = Some(this.edit_revision);
                    this.scroll_animation = None;
                    this.signature = None;
                    this.service.update(
                        PathBuf::from(&path),
                        source,
                        Some(EditLocation {
                            revision: this.edit_revision,
                            path: path.into(),
                            line: position.line,
                            character: position.character,
                        }),
                    );
                    this.compiling = true;
                    this.schedule_autosave(index, cx);
                }
                cx.notify();
            },
        );
        let observer = cx.observe(&editor, move |this, editor, cx| {
            // Mouse placement can leave a Normal-mode caret just past the last
            // grapheme. Keep the native selection and the modal cursor aligned.
            if this.settings.vim_enabled && this.documents[index].vim.mode == vim::Mode::Normal {
                let state = editor.read(cx);
                let cursor = state.cursor();
                let at = vim::motion::clip(&state.value(), cursor);
                if state.selected_range().is_empty() && at != cursor {
                    editor.update(cx, |state, cx| state.set_selected_range(at..at, cx));
                }
            }
            cx.notify();
        });
        let mut vim = vim::State::default();
        vim.reset(&source, 0);
        (
            Document {
                path,
                uri,
                editor,
                saved: source,
                dirty: false,
                save_error: None,
                autosave_task: None,
                vim,
            },
            vec![changes, observer],
        )
    }

    pub(super) fn ensure_document(
        &mut self,
        uri: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<usize> {
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.uri == uri)
        {
            return Ok(index);
        }
        let path = lsp::uri_path(uri)?;
        let source = fs::read_to_string(&path)?;
        let label = path
            .strip_prefix(&self.root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        let index = self.documents.len();
        let (document, subscriptions) =
            Self::make_document(index, label, source, &self.root, &self.lsp, window, cx);
        Self::configure_document(&document, &self.settings, window, cx);
        self.documents.push(document);
        self._subscriptions.extend(subscriptions);
        Ok(index)
    }

    pub(super) fn navigate(
        &mut self,
        uri: &str,
        range: Option<lsp_types::Range>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let index = self.ensure_document(uri, window, cx)?;
        self.open_document(index, window, cx);
        if let Some(range) = range {
            self.documents[index].editor.update(cx, |state, cx| {
                let text = state.value();
                let start = lsp::position_byte(&text, range.start).unwrap_or(0);
                let end = lsp::position_byte(&text, range.end).unwrap_or(start);
                state.set_cursor_position(lsp::editor_position(&text, range.start), window, cx);
                if self.settings.vim_enabled && self.documents[index].vim.mode != vim::Mode::Insert
                {
                    let start = vim::motion::clip(&text, start);
                    state.set_selected_range(start..start, cx);
                } else {
                    state.set_selected_range(start..end, cx);
                }
            });
        }
        Ok(())
    }

    pub(super) fn apply_workspace_edit(
        &mut self,
        edit: lsp_types::WorkspaceEdit,
        expected: Option<&std::collections::HashMap<String, lsp::Snapshot>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context as _, bail};
        use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf};
        let mut files =
            std::collections::BTreeMap::<String, (Option<i32>, Vec<lsp_types::TextEdit>)>::new();
        for (uri, edits) in edit.changes.unwrap_or_default() {
            files.entry(uri.to_string()).or_default().1.extend(edits);
        }
        if let Some(changes) = edit.document_changes {
            let changes = match changes {
                DocumentChanges::Edits(edits) => edits,
                DocumentChanges::Operations(operations) => operations
                    .into_iter()
                    .map(|operation| match operation {
                        DocumentChangeOperation::Edit(edit) => Ok(edit),
                        DocumentChangeOperation::Op(_) => Err(anyhow::anyhow!(
                            "File creation, deletion, and renaming are not supported by this action"
                        )),
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            };
            for document in changes {
                let entry = files
                    .entry(document.text_document.uri.to_string())
                    .or_default();
                entry.0 = document.text_document.version;
                entry
                    .1
                    .extend(document.edits.into_iter().map(|edit| match edit {
                        OneOf::Left(edit) => edit,
                        OneOf::Right(edit) => edit.text_edit,
                    }));
            }
        }
        let mut planned = Vec::new();
        for (uri, (version, edits)) in files {
            let path = lsp::uri_path(&uri)?;
            if !path.starts_with(&self.root) {
                bail!("The language server tried to edit a file outside this project");
            }
            let snapshot = self.lsp.snapshot(&uri);
            if let Some(expected) = expected.and_then(|snapshots| snapshots.get(&uri))
                && snapshot
                    .as_ref()
                    .is_none_or(|current| current.version != expected.version)
            {
                bail!("The document changed while this action was running. Try again.");
            }
            if let Some(version) = version
                && snapshot
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.version != version)
            {
                bail!("The language server edit refers to an older document version");
            }
            let text = match snapshot {
                Some(snapshot) => snapshot.text,
                None => fs::read_to_string(&path)
                    .with_context(|| format!("Could not read {}", path.display()))?,
            };
            planned.push((uri, lsp::edited_text(&text, &edits)?));
        }
        // Load every buffer before applying anything, so failures cannot leave
        // a cross-file rename half-applied. Nothing is written to disk.
        let mut buffers = Vec::new();
        for (uri, text) in planned {
            let index = self.ensure_document(&uri, window, cx)?;
            buffers.push((index, text));
        }
        for (index, text) in buffers {
            if !self.tabs.contains(&index) {
                self.tabs.push(index);
            }
            self.documents[index].editor.update(cx, |state, cx| {
                let position = state.cursor_position();
                state.replace_all(text, window, cx);
                let last_line = state.value().bytes().filter(|&byte| byte == b'\n').count() as u32;
                state.set_cursor_position(
                    lsp_types::Position::new(position.line.min(last_line), position.character),
                    window,
                    cx,
                );
            });
        }
        // A delayed edit can reopen a buffer after the last visible tab was
        // closed. Restore a valid active tab before focusing an editor.
        if !self.tabs.contains(&self.active)
            && let Some(&index) = self.tabs.first()
        {
            self.open_document(index, window, cx);
        }
        if !self.tabs.is_empty() {
            self.documents[self.active]
                .editor
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
        Ok(())
    }

    pub(super) fn format_document(
        &mut self,
        _: &FormatDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        let document = &self.documents[self.active];
        let uri = document.uri.clone();
        self.lsp
            .sync(&uri, document.editor.read(cx).value().as_ref());
        let tab_width = self.settings.tab_width;
        // Tinymist's typstyle formatter uses its configuration rather than the
        // LSP FormattingOptions for indentation. Keep both in agreement.
        self.lsp.notify(
            "workspace/didChangeConfiguration",
            serde_json::json!({ "settings": { "formatterIndentSize": tab_width } }),
        );
        let versions = self.lsp.snapshots();
        let client = self.lsp.clone();
        self.language_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = client.request("textDocument/formatting", serde_json::json!({ "textDocument": { "uri": uri }, "options": { "tabSize": tab_width, "insertSpaces": true } })).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let result = result.and_then(|value| {
                    let edits: Vec<lsp_types::TextEdit> = if value.is_null() { vec![] } else { serde_json::from_value(value)? };
                    if edits.is_empty() { return Ok(()); }
                    let edit = serde_json::from_value(serde_json::json!({ "changes": { uri: edits } }))?;
                    this.apply_workspace_edit(edit, Some(&versions), window, cx)
                });
                this.lsp_error = result.err().map(|error| error.to_string());
                cx.notify();
            });
        }));
    }

    pub(super) fn rename_symbol(
        &mut self,
        _: &RenameSymbol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        let document = &self.documents[self.active];
        let source = document.editor.read(cx).value().to_string();
        let position = lsp::byte_position(&source, document.editor.read(cx).cursor());
        let uri = document.uri.clone();
        self.lsp.sync(&uri, &source);
        let versions = self.lsp.snapshots();
        let client = self.lsp.clone();
        self.language_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = client
                .request(
                    "textDocument/prepareRename",
                    serde_json::json!({ "textDocument": { "uri": uri }, "position": position }),
                )
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let result = result.and_then(|value| {
                    if value.is_null() {
                        anyhow::bail!("There is no renameable symbol at the cursor");
                    }
                    if versions.iter().any(|(uri, snapshot)| {
                        this.lsp
                            .snapshot(uri)
                            .is_none_or(|current| current.version != snapshot.version)
                    }) {
                        anyhow::bail!("The document changed. Request rename again.");
                    }
                    let name = if let Some(name) = value["placeholder"].as_str() {
                        name.to_owned()
                    } else {
                        let range: lsp_types::Range =
                            serde_json::from_value(value.get("range").unwrap_or(&value).clone())?;
                        source[lsp::position_byte(&source, range.start)?
                            ..lsp::position_byte(&source, range.end)?]
                            .to_owned()
                    };
                    this.rename_target = Some(RenameTarget {
                        uri,
                        position,
                        versions,
                    });
                    this.rename_input.update(cx, |state, cx| {
                        state.set_value(name, window, cx);
                        state.focus(window, cx);
                        state.set_selected_range(0..state.value().len(), cx);
                    });
                    Ok(())
                });
                this.lsp_error = result.err().map(|error| error.to_string());
                cx.notify();
            });
        }));
    }

    pub(super) fn finish_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.rename_target.take() else {
            return;
        };
        let name = self.rename_input.read(cx).value().to_string();
        if name.trim().is_empty() {
            self.rename_target = Some(target);
            return;
        }
        let client = self.lsp.clone();
        self.language_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = client.request("textDocument/rename", serde_json::json!({ "textDocument": { "uri": target.uri }, "position": target.position, "newName": name })).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let result = result.and_then(|value| {
                    if value.is_null() { anyhow::bail!("Tinymist returned no rename edits"); }
                    this.apply_workspace_edit(serde_json::from_value(value)?, Some(&target.versions), window, cx)
                });
                this.lsp_error = result.err().map(|error| error.to_string());
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn find_references(
        &mut self,
        _: &FindReferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position_request("textDocument/references", window, cx);
    }

    pub(super) fn signature_help(
        &mut self,
        _: &SignatureHelp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position_request("textDocument/signatureHelp", window, cx);
    }

    pub(super) fn position_request(
        &mut self,
        method: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        let document = &self.documents[self.active];
        let text = document.editor.read(cx).value();
        let offset = document.editor.read(cx).cursor();
        self.lsp.sync(&document.uri, &text);
        let uri = document.uri.clone();
        let version = self.lsp.snapshot(&uri).map(|snapshot| snapshot.version);
        let params = serde_json::json!({ "textDocument": { "uri": uri }, "position": lsp::byte_position(&text, offset), "context": if method.ends_with("references") { serde_json::json!({ "includeDeclaration": true }) } else { serde_json::json!({ "triggerKind": 1, "isRetrigger": false }) } });
        let client = self.lsp.clone();
        self.language_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = client.request(method, params).await;
            let _ = this.update(cx, |this, cx| {
                if this.lsp.snapshot(&uri).map(|snapshot| snapshot.version) != version
                    || this.documents[this.active].uri != uri
                {
                    return;
                }
                let result = result.and_then(|value| {
                    if method.ends_with("references") {
                        this.references = Some(if value.is_null() {
                            vec![]
                        } else {
                            serde_json::from_value(value)?
                        });
                    } else {
                        let signature: Option<lsp_types::SignatureHelp> =
                            serde_json::from_value(value)?;
                        this.signature = signature.and_then(|help| {
                            help.signatures
                                .get(help.active_signature.unwrap_or(0) as usize)
                                .map(|signature| signature.label.clone())
                        });
                    }
                    Ok(())
                });
                this.lsp_error = result.err().map(|error| error.to_string());
                cx.notify();
            });
        }));
    }

    pub(super) fn language_panels(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .w_full()
            .when(self.rename_target.is_some(), |this| {
                this.child(
                    h_flex()
                        .p_2()
                        .gap_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.rename_input)
                                    .small()
                                    .aria_label("Rename symbol"),
                            ),
                        )
                        .child(
                            Button::new("confirm-rename")
                                .small()
                                .label("Rename")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_rename(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cancel-rename")
                                .small()
                                .ghost()
                                .icon(IconName::X)
                                .tooltip("Cancel rename")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.rename_target = None;
                                    this.documents[this.active]
                                        .editor
                                        .update(cx, |state, cx| state.focus(window, cx));
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.signature.as_ref(), |this, signature| {
                this.child(
                    h_flex()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .child(signature.clone()),
                        )
                        .child(
                            Button::new("close-signature")
                                .xsmall()
                                .ghost()
                                .icon(IconName::X)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.signature = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.references.as_ref(), |this, locations| {
                this.child(
                    v_flex()
                        .max_h(px(240.))
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .px_3()
                                .py_1()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.))
                                        .child(format!("{} references", locations.len())),
                                )
                                .child(
                                    Button::new("close-references")
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::X)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.references = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            v_flex()
                                .id("references-results")
                                .overflow_y_scroll()
                                .children(locations.iter().enumerate().map(|(index, location)| {
                                    let uri = location.uri.to_string();
                                    let range = location.range;
                                    let label = lsp::uri_path(&uri)
                                        .map(|path| {
                                            path.strip_prefix(&self.root)
                                                .unwrap_or(&path)
                                                .display()
                                                .to_string()
                                        })
                                        .unwrap_or_else(|_| uri.clone());
                                    ListItem::new(("reference", index))
                                        .px_3()
                                        .py_1()
                                        .text_size(px(12.))
                                        .child(div().truncate().child(format!(
                                            "{}:{}:{}",
                                            label,
                                            range.start.line + 1,
                                            range.start.character + 1
                                        )))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if let Err(error) =
                                                this.navigate(&uri, Some(range), window, cx)
                                            {
                                                this.lsp_error = Some(error.to_string());
                                            }
                                            cx.notify();
                                        }))
                                })),
                        ),
                )
            })
    }
}
