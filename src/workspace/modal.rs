use super::*;
use crate::{
    ToggleVim,
    vim::{self, Effect, Mode, Patch},
};

pub(super) fn key_name(key: &Keystroke) -> String {
    if key.modifiers.platform {
        return format!("cmd-{}", key.key);
    }
    // Reserve non-printing Ctrl+Alt combinations for application shortcuts.
    // Printable AltGr input still follows key_char below.
    if key.modifiers.control && key.modifiers.alt && key.key_char.is_none() {
        return format!(
            "ctrl-alt-{}{}",
            if key.modifiers.shift { "shift-" } else { "" },
            key.key
        );
    }
    // AltGr produces printable Unicode on international keyboard layouts.
    if key.modifiers.alt && !key.modifiers.control {
        return format!("alt-{}", key.key);
    }
    if key.modifiers.control && !key.modifiers.alt {
        return format!(
            "ctrl-{}{}",
            if key.modifiers.shift { "shift-" } else { "" },
            key.key
        );
    }
    if let Some(ch) = &key.key_char {
        return ch.clone();
    }
    if key.modifiers.shift && key.key.len() == 1 {
        return match key.key.as_str() {
            "1" => "!",
            "2" => "@",
            "3" => "#",
            "4" => "$",
            "5" => "%",
            "6" => "^",
            "7" => "&",
            "8" => "*",
            "9" => "(",
            "0" => ")",
            "-" => "_",
            "=" => "+",
            "[" => "{",
            "]" => "}",
            ";" => ":",
            "'" => "\"",
            "," => "<",
            "." => ">",
            "/" => "?",
            "`" => "~",
            "\\" => "|",
            _ => return key.key.to_uppercase(),
        }
        .into();
    }
    key.key.clone()
}

impl Workspace {
    pub(super) fn toggle_vim(
        &mut self,
        _: &ToggleVim,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.vim_enabled = !self.settings.vim_enabled;
        self.vim_command = None;
        for document in &mut self.documents {
            document.editor.update(cx, |state, cx| {
                let text = state.value();
                let cursor = state.cursor();
                document.vim.reset(&text, cursor);
                state.set_readonly(self.settings.vim_enabled, cx);
                if self.settings.vim_enabled {
                    let caret = vim::motion::clip(&text, cursor);
                    state.set_selected_range(caret..caret, cx);
                    state.dismiss_lsp_overlays(cx);
                }
            });
        }
        if !self.settings_open {
            self.documents[self.active]
                .editor
                .update(cx, |state, cx| state.focus(window, cx));
        }
        self.save_settings(cx);
    }
    pub(super) fn vim_key(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.settings.vim_enabled || self.tabs.is_empty() {
            return false;
        }
        let key = key_name(&event.keystroke);
        if self.vim_command.is_some() && self.vim_input.read(cx).focus_handle(cx).is_focused(window)
        {
            if matches!(key.as_str(), "enter" | "\n") {
                self.finish_vim_command(window, cx);
                return true;
            }
            if matches!(key.as_str(), "escape" | "ctrl-[" | "ctrl-c") {
                self.vim_command = None;
                self.documents[self.active]
                    .editor
                    .update(cx, |state, cx| state.focus(window, cx));
                cx.notify();
                return true;
            }
            return false;
        }
        let editor = self.documents[self.active].editor.clone();
        let state = editor.read(cx);
        if !state.focus_handle(cx).is_focused(window) {
            return false;
        }
        let insert_mode = self.documents[self.active].vim.mode == Mode::Insert;
        let escape = matches!(key.as_str(), "escape" | "ctrl-[" | "ctrl-c");
        if insert_mode && !escape {
            return false;
        }
        if (state.code_action_menu_state().open || state.completion_menu_state().open)
            && matches!(key.as_str(), "up" | "down" | "enter" | "\n" | "tab")
        {
            return false;
        }
        let text = state.value().to_string();
        let cursor = state.cursor();
        let selection = state.selected_range();
        if self.documents[self.active].vim.needs_clipboard(&key)
            && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
        {
            self.vim_registers.clipboard(text);
        }
        self.documents[self.active]
            .vim
            .set_indent_width(usize::from(self.settings.tab_width));
        let Some(result) = self.documents[self.active].vim.key(
            &key,
            &text,
            cursor,
            selection,
            &mut self.vim_registers,
        ) else {
            return false;
        };
        if escape {
            self.references = None;
            self.signature = None;
            self.rename_target = None;
            self.language_task = None;
        }
        let insert = self.documents[self.active].vim.mode == Mode::Insert;
        editor.update(cx, |state, cx| {
            if result.text != text {
                let patch = Patch::between(&text, &result.text);
                state.set_selected_range(patch.start..patch.start + patch.removed.len(), cx);
                state.replace(&patch.inserted, window, cx);
            }
            state.set_selected_range(result.selection, cx);
            state.set_readonly(!insert, cx);
            if !insert || matches!(key.as_str(), "escape" | "ctrl-[" | "ctrl-c") {
                state.dismiss_lsp_overlays(cx);
            }
        });
        if let Some(text) = self.vim_registers.clipboard_write.take() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        match result.effect {
            Some(Effect::Command(prefix)) => {
                self.vim_command = Some(prefix);
                self.vim_input.update(cx, |state, cx| {
                    state.set_value("", window, cx);
                    state.focus(window, cx);
                });
            }
            Some(Effect::Definition) => {
                window.dispatch_action(Box::new(gpui_kit::component::input::GoToDefinition), cx)
            }
            Some(Effect::References) => self.find_references(&FindReferences, window, cx),
            Some(Effect::Hover) => self.vim_hover(window, cx),
            None => {}
        }
        cx.notify();
        true
    }
    fn vim_hover(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let editor = self.documents[self.active].editor.clone();
        let state = editor.read(cx);
        let provider = crate::lsp_provider::Provider::new(
            self.lsp.clone(),
            self.documents[self.active].uri.clone(),
        );
        let text = state.text().clone();
        let cursor = state.cursor();
        let snapshot = state.value();
        let range = vim::motion::word_object(&snapshot, cursor, false, false, 1);
        let task = provider.hover_now(&text, cursor, cx);
        self.language_task = Some(cx.spawn(async move |_, cx| {
            if let Ok(Some(hover)) = task.await {
                editor.update(cx, |state, cx| {
                    if state.value() == snapshot && state.cursor() == cursor {
                        state.present_hover(range, hover, cx);
                    }
                });
            }
        }));
    }
    pub(super) fn finish_vim_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prefix) = self.vim_command.take() else {
            return;
        };
        let command = self.vim_input.read(cx).value().to_string();
        let editor = self.documents[self.active].editor.clone();
        editor.update(cx, |state, cx| state.focus(window, cx));
        if matches!(prefix, '/' | '?') {
            let text = editor.read(cx).value();
            match self.documents[self.active]
                .vim
                .search(&text, &command, prefix == '/')
            {
                Ok(range) => editor.update(cx, |state, cx| state.set_selected_range(range, cx)),
                Err(error) => self.documents[self.active].vim.message = Some(error),
            }
        } else {
            let command = command.trim();
            match command {
                "" => {}
                "w" | "write" => self.save(&Save, window, cx),
                "wa" | "wall" => {
                    let active = self.active;
                    for index in 0..self.documents.len() {
                        if self.documents[index].dirty {
                            self.active = index;
                            self.save(&Save, window, cx);
                        }
                    }
                    self.active = active;
                }
                "q" | "quit" => {
                    if self.documents[self.active].dirty {
                        self.documents[self.active].vim.message = Some(
                            "Unsaved changes; :w to save or :q! to close and keep the buffer"
                                .into(),
                        );
                    } else {
                        self.close_tab(self.active, window, cx);
                    }
                }
                "q!" => self.close_tab(self.active, window, cx),
                "wq" | "x" => {
                    self.save(&Save, window, cx);
                    if self.save_error.is_none() {
                        self.close_tab(self.active, window, cx);
                    }
                }
                "bn" | "bnext" | "bp" | "bprevious" => {
                    if let Some(position) = self.tabs.iter().position(|index| *index == self.active)
                    {
                        let next = if matches!(command, "bp" | "bprevious") {
                            (position + self.tabs.len() - 1) % self.tabs.len()
                        } else {
                            (position + 1) % self.tabs.len()
                        };
                        self.open_document(self.tabs[next], window, cx);
                    }
                }
                "set novim" => self.toggle_vim(&ToggleVim, window, cx),
                "noh" | "nohlsearch" => self.documents[self.active].vim.message = None,
                "format" => self.format_document(&FormatDocument, window, cx),
                _ => {
                    if let Ok(line) = command.parse::<usize>() {
                        let text = editor.read(cx).value();
                        let range = self.documents[self.active].vim.goto_line(&text, line);
                        editor.update(cx, |state, cx| state.set_selected_range(range, cx));
                    } else if let Some(path) = command
                        .strip_prefix("e ")
                        .or_else(|| command.strip_prefix("edit "))
                    {
                        let path = self.root.join(path.trim());
                        match self.navigate(&lsp::file_uri(&path), None, window, cx) {
                            Ok(()) => {}
                            Err(error) => {
                                self.documents[self.active].vim.message = Some(error.to_string())
                            }
                        }
                    } else {
                        self.documents[self.active].vim.message =
                            Some(format!("Unknown command: {command}"));
                    }
                }
            }
        }
        cx.notify();
    }
    pub(super) fn vim_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .w_full()
            .when_some(self.vim_command, |this, prefix| {
                this.child(
                    h_flex()
                        .px_3()
                        .py_1()
                        .gap_1()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(prefix.to_string())
                        .child(
                            div().flex_1().child(
                                Input::new(&self.vim_input)
                                    .small()
                                    .aria_label("Vim command"),
                            ),
                        ),
                )
            })
            .when_some(
                self.settings
                    .vim_enabled
                    .then(|| self.documents[self.active].vim.message.as_ref())
                    .flatten(),
                |this, message| {
                    this.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(message.clone()),
                    )
                },
            )
    }
}

impl Workspace {
    pub(super) fn vim_caret(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.documents[self.active].editor.clone();
        let visual = self.documents[self.active].vim.mode.visual();
        let visual_caret = self.documents[self.active].vim.caret();
        let font_size = px(f32::from(self.settings.font_size));
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let state = editor.read(cx);
                if !state.focus_handle(cx).is_focused(window) {
                    return;
                }
                let text = state.value();
                let at = if visual {
                    visual_caret.min(text.len())
                } else {
                    state.cursor().min(text.len())
                };
                let next = vim::motion::next(&text, at);
                let glyph = state.range_to_bounds(&(at..next));
                let Some(geometry) = state.prepaint_caret_geometry() else {
                    return;
                };
                let height = geometry.line_height();
                let mut caret = geometry.cursor_bounds();
                caret.origin.y += geometry.scroll_offset().y;
                let mut bounds = caret;
                bounds.origin.y -= (height - caret.size.height) / 2.;
                bounds.size.height = height;
                let glyph_width = glyph
                    .map(|g| g.size.width)
                    .filter(|w| *w > px(0.) && *w < font_size * 4.);
                bounds.size.width = glyph_width.unwrap_or(font_size * 0.6).max(caret.size.width);
                if visual && let Some(glyph) = glyph {
                    bounds.origin = glyph.origin;
                }
                let glyph_origin = bounds.origin + point(px(1.), px(0.));
                let bounds = bounds.intersect(&state.input_bounds());
                if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                    // Opaque fill covers the native thin caret in both blink
                    // phases. Redraw the covered grapheme with inverted ink,
                    // just as Zed does for its block cursor.
                    window.with_content_mask(
                        Some(ContentMask {
                            bounds: state.input_bounds(),
                        }),
                        |window| {
                            if visual && !bounds.contains(&caret.origin) {
                                window.paint_quad(fill(
                                    caret,
                                    cx.theme()
                                        .highlight_theme
                                        .style
                                        .editor_background
                                        .unwrap_or(cx.theme().background),
                                ));
                            }
                            window.paint_quad(fill(bounds, cx.theme().primary));
                            let glyph = &text[at..next];
                            if !glyph.chars().all(char::is_whitespace) {
                                let line = window.text_system().shape_line(
                                    glyph.to_owned().into(),
                                    font_size,
                                    &[TextRun {
                                        len: glyph.len(),
                                        font: Font {
                                            family: cx.theme().mono_font_family.clone(),
                                            ..Default::default()
                                        },
                                        color: cx
                                            .theme()
                                            .highlight_theme
                                            .style
                                            .editor_background
                                            .unwrap_or(cx.theme().background),
                                        ..Default::default()
                                    }],
                                    None,
                                );
                                let _ = line.paint(
                                    glyph_origin,
                                    height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        },
                    );
                }
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    }
}
