use super::*;

impl Workspace {
    pub(super) fn status_key(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.goto_open || !self.goto_input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        match super::modal::key_name(&event.keystroke).as_str() {
            "enter" | "\n" => self.finish_goto(window, cx),
            "escape" | "ctrl-[" | "ctrl-c" => {
                self.goto_open = false;
                self.documents[self.active]
                    .editor
                    .update(cx, |state, cx| state.focus(window, cx));
                cx.notify();
            }
            _ => return false,
        }
        true
    }
    pub(super) fn go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        self.goto_open = true;
        self.goto_error = None;
        self.vim_command = None;
        let position = self.documents[self.active]
            .editor
            .read(cx)
            .cursor_position();
        self.goto_input.update(cx, |state, cx| {
            state.set_value(
                format!("{}:{}", position.line + 1, position.character + 1),
                window,
                cx,
            );
            state.select_all(window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }
    pub(super) fn finish_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.goto_open {
            return;
        }
        let value = self.goto_input.read(cx).value();
        let mut parts = value.trim().split(':');
        let line = parts.next().and_then(|part| part.parse::<u32>().ok());
        let column = parts
            .next()
            .map_or(Some(1), |part| part.parse::<u32>().ok());
        let valid = line
            .zip(column)
            .filter(|(line, column)| *line > 0 && *column > 0 && parts.next().is_none());
        let Some((line, column)) = valid else {
            self.goto_error = Some("Enter a positive line or line:column".into());
            cx.notify();
            return;
        };
        let document = &mut self.documents[self.active];
        document.editor.update(cx, |state, cx| {
            state.set_cursor_position(lsp_types::Position::new(line - 1, column - 1), window, cx);
            if self.settings.vim_enabled && document.vim.mode != vim::Mode::Insert {
                let text = state.value();
                let at = vim::motion::clip(&text, state.cursor());
                document.vim.goto_line(&text, line as usize);
                state.set_selected_range(at..at, cx);
            }
            state.focus(window, cx);
        });
        self.goto_open = false;
        cx.notify();
    }
    pub(super) fn toggle_wrap(
        &mut self,
        _: &ToggleWrap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.soft_wrap = !self.settings.soft_wrap;
        for document in &self.documents {
            document.editor.update(cx, |state, cx| {
                state.set_soft_wrap(self.settings.soft_wrap, window, cx)
            });
        }
        self.save_settings(cx);
    }
    pub(super) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(false, window, cx);
    }
    pub(super) fn previous_tab(
        &mut self,
        _: &PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab(true, window, cx);
    }
    fn cycle_tab(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|&index| index == self.active) {
            let next = (index + if backward { self.tabs.len() - 1 } else { 1 }) % self.tabs.len();
            self.open_document(self.tabs[next], window, cx);
        }
    }
    pub(super) fn goto_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().flex_shrink_0().when(self.goto_open, |this| {
            this.child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child("Go to")
                    .child(
                        div().w(px(160.)).child(
                            Input::new(&self.goto_input)
                                .small()
                                .aria_label("Go to line"),
                        ),
                    )
                    .when_some(self.goto_error.as_ref(), |this, error| {
                        this.child(
                            div()
                                .text_color(cx.theme().danger)
                                .text_size(px(11.))
                                .child(error.clone()),
                        )
                    })
                    .child(
                        Button::new("goto-confirm")
                            .ghost()
                            .small()
                            .label("Go")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.finish_goto(window, cx)),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Enter to go · Esc to cancel"),
                    ),
            )
        })
    }
    pub(super) fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let document = &self.documents[self.active];
        let state = document.editor.read(cx);
        let position = state.cursor_position();
        let selected = state.selected_range();
        let selection = if selected.is_empty() {
            None
        } else {
            Some(format!(
                "{} selected",
                state.value()[selected].chars().count()
            ))
        };
        let diagnostics = self.lsp.diagnostics(&document.uri);
        let errors = diagnostics
            .iter()
            .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
            .count();
        let warnings = diagnostics
            .iter()
            .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING))
            .count();
        h_flex()
            .id("status-bar")
            .test_support()
            .h(px(30.))
            .flex_shrink_0()
            .px(px(9.))
            .gap(px(9.))
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tab_bar)
            .text_size(px(12.))
            .text_color(cx.theme().muted_foreground)
            .when(!self.library_editor, |this| {
                this.child(
                    Button::new("toggle-sidebar")
                        .ghost()
                        .xsmall()
                        .w(px(22.))
                        .h(px(22.))
                        .px_0()
                        .child(Icon::new(IconName::PanelLeft).size(px(14.)))
                        .selected(self.sidebar_visible)
                        .tooltip(crate::runtime::shortcut("Toggle file tree · Ctrl+B"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_sidebar(&ToggleSidebar, window, cx)
                        })),
                )
            })
            .when(!self.tabs.is_empty(), |this| {
                this.child(
                    div()
                        .max_w(px(220.))
                        .truncate()
                        .child(document.path.clone()),
                )
            })
            .when(errors + warnings > 0, |this| {
                this.child(diagnostics::summary(errors, warnings))
            })
            .child(div().flex_1())
            .when_some(selection, |this, selection| this.child(selection))
            .when(self.settings.vim_enabled && !self.tabs.is_empty(), |this| {
                let color = match document.vim.mode {
                    vim::Mode::Insert => rgb(0x238636),
                    vim::Mode::Normal => rgb(0x3973b8),
                    vim::Mode::Visual | vim::Mode::VisualLine => rgb(0x8b5fbf),
                };
                this.child(
                    div()
                        .id("vim-mode")
                        .test_support()
                        .rounded(px(3.))
                        .px_2()
                        .py(px(2.))
                        .bg(color)
                        .text_color(rgb(0xffffff))
                        .text_size(px(11.))
                        .child(document.vim.status()),
                )
            })
            .child(
                Button::new("cursor-position")
                    .ghost()
                    .xsmall()
                    .disabled(self.tabs.is_empty())
                    .h(px(22.))
                    .child(div().text_size(px(12.)).child(format!(
                        "Ln {}, Col {}",
                        position.line + 1,
                        position.character + 1
                    )))
                    .tooltip(crate::runtime::shortcut("Go to line · Ctrl+G"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.go_to_line(&GoToLine, window, cx)),
                    ),
            )
            .child(
                Button::new("toggle-preview")
                    .ghost()
                    .xsmall()
                    .w(px(22.))
                    .h(px(22.))
                    .px_0()
                    .child(Icon::new(IconName::PanelRight).size(px(14.)))
                    .selected(self.preview_visible || self.detached_preview.is_some())
                    .tooltip(crate::runtime::shortcut(
                        if self.detached_preview.is_some() {
                            "Return preview to editor"
                        } else {
                            "Toggle preview · Ctrl+Alt+P"
                        },
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_preview(&TogglePreview, window, cx)
                    })),
            )
    }
}
