use super::*;
use gpui_kit::component::menu::DropdownMenu;
impl Workspace {
    pub(super) fn reveal_source(
        &mut self,
        _: &crate::RevealSource,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.is_empty() {
            cx.reveal_path(&self.root.join(&self.documents[self.active].path));
        }
    }

    pub(crate) fn prepare_to_leave(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.export_busy {
            self.export_message = Some("Finishing PDF export…".into());
            cx.notify();
            return false;
        }
        if !self.save_all(window, cx) {
            return false;
        }
        self.dock_preview(window, cx);
        true
    }

    pub(super) fn schedule_autosave(&mut self, index: usize, cx: &mut Context<Self>) {
        self.documents[index].autosave_task = None;
        if !self.settings.autosave || !self.documents[index].dirty {
            return;
        }
        self.documents[index].autosave_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(750))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.settings.autosave && this.documents[index].dirty {
                    this.save_document(index, cx);
                }
            });
        }));
    }

    pub(super) fn close_project(
        &mut self,
        _: &crate::CloseProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_close(window, cx) {
            window.remove_window();
        }
    }

    fn save_all(&mut self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        for index in 0..self.documents.len() {
            if !self.documents[index].dirty {
                continue;
            }
            self.documents[index].autosave_task = None;
            if !self.save_document(index, cx) {
                return false;
            }
        }
        true
    }
    pub(super) fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.documents.iter().any(|doc| doc.dirty) {
            return true;
        }
        if self.closing_prompt {
            return false;
        }
        if self.settings.autosave && self.save_all(window, cx) {
            return true;
        }
        self.closing_prompt = true;
        let response = window.prompt(
            PromptLevel::Warning,
            "Save your changes before closing?",
            Some("Your open documents contain unsaved changes."),
            &["Save all", "Discard changes", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = response.await.ok();
            let _ = this.update_in(cx, |this, window, cx| {
                this.closing_prompt = false;
                if answer == Some(1) || (answer == Some(0) && this.save_all(window, cx)) {
                    window.remove_window();
                }
                cx.notify();
            });
        })
        .detach();
        false
    }
    pub(super) fn export_menu(&self, _: &mut Context<Self>) -> impl IntoElement {
        let library = crate::invoice::is_library_document(&self.root, &self.entry);
        let busy = self.export_busy;
        Button::new("export-share-menu")
            .ghost()
            .small()
            .icon(IconName::ChevronDown)
            .tooltip("PDF actions")
            .dropdown_menu(move |menu, _, _| {
                menu.menu_with_disabled("Email PDF…", Box::new(crate::EmailPdf), busy)
                    .menu_with_disabled("Share PDF…", Box::new(crate::SharePdf), busy)
                    .when(library, |menu| {
                        menu.separator()
                            .menu("PDF versions", Box::new(crate::PdfHistory))
                    })
            })
    }
    pub(super) fn export_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.export_with(crate::share::Intent::Keep, window, cx);
    }
    pub(super) fn export_with(
        &mut self,
        intent: crate::share::Intent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.export_busy || !self.save_all(window, cx) {
            return;
        }
        self.export_busy = true;
        self.export_message = None;
        self.export_path = None;
        let library = crate::invoice::is_library_document(&self.root, &self.entry);
        let suggested = format!(
            "{}.pdf",
            std::path::Path::new(&self.entry)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        );
        let dialog = if library {
            None
        } else {
            Some(cx.prompt_for_new_path(&self.root, Some(&suggested)))
        };
        let root = self.root.clone();
        let entry = self.entry.clone();
        let subject = std::path::Path::new(&entry)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        cx.spawn_in(window, async move |this, cx| {
            let result = if let Some(dialog) = dialog {
                let destination = match dialog.await {
                    Ok(Ok(Some(path))) => path.with_extension("pdf"),
                    Ok(Ok(None)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.export_busy = false;
                            cx.notify();
                        });
                        return;
                    }
                    result => {
                        let _ = this.update(cx, |this, cx| {
                            this.export_busy = false;
                            this.export_message =
                                Some(format!("Could not open save dialog: {result:?}"));
                            cx.notify();
                        });
                        return;
                    }
                };
                cx.background_executor()
                    .spawn(async move {
                        crate::project::export_pdf(&root, &entry, &destination)
                            .map(|()| destination)
                    })
                    .await
            } else {
                cx.background_executor()
                    .spawn(async move {
                        crate::invoice::archive::export(&root, &entry).map(|version| version.pdf)
                    })
                    .await
            };
            match result {
                Ok(path) => {
                    let attached = path.clone();
                    let outcome = cx
                        .background_executor()
                        .spawn(
                            async move { crate::share::handoff(&attached, intent, &subject).await },
                        )
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.export_busy = false;
                        this.export_path = Some(path.clone());
                        this.export_message = Some(match outcome {
                            Ok(()) => format!("PDF saved to {}", path.display()),
                            Err(error) => format!("PDF retained at {}; {error}", path.display()),
                        });
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.export_busy = false;
                        this.export_message = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }
}
