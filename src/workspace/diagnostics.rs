use super::*;
use gpui_kit::base::input::{DiagnosticSeverity, RopeExt};

pub(super) fn severity_color(severity: lsp_types::DiagnosticSeverity) -> Hsla {
    if severity == lsp_types::DiagnosticSeverity::ERROR {
        rgb(0xb42318).into()
    } else {
        rgb(0x946200).into()
    }
}
pub(super) fn file_severity(
    diagnostics: &[lsp_types::Diagnostic],
) -> Option<lsp_types::DiagnosticSeverity> {
    if diagnostics
        .iter()
        .any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
    {
        Some(lsp_types::DiagnosticSeverity::ERROR)
    } else if diagnostics
        .iter()
        .any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING))
    {
        Some(lsp_types::DiagnosticSeverity::WARNING)
    } else {
        None
    }
}
pub(super) fn summary(errors: usize, warnings: usize) -> String {
    [(errors, "error"), (warnings, "warning")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, kind)| format!("{count} {kind}{}", if count == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(" · ")
}

impl Workspace {
    pub(super) fn delay_diagnostic_hover(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.documents[self.active].editor.clone();
        let state = editor.read(cx);
        let inside = state.input_bounds().contains(&event.position);
        let diagnostic = state.diagnostic_popover().filter(|_| inside);
        if let Some(entry) = &diagnostic
            && self
                .diagnostic_hover
                .as_ref()
                .is_some_and(|(index, shown)| {
                    *index == self.active && shown.as_ref() == entry.as_ref()
                })
        {
            return;
        }
        let scroll = state.scroll_offset();
        self.diagnostic_hover_task = None;
        self.diagnostic_hover = None;
        // Kit presents diagnostics on mouse move without a delay. This parent
        // handler runs after the editor, before a frame can show that popover.
        editor.update(cx, |state, cx| state.clear_diagnostic_popover(cx));
        let Some(entry) = diagnostic else {
            return;
        };
        let index = self.active;
        let revision = self.edit_revision;
        self.diagnostic_hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(650))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.active != index
                    || this.edit_revision != revision
                    || editor.read(cx).scroll_offset() != scroll
                    || !editor
                        .read(cx)
                        .diagnostics()
                        .is_some_and(|set| set.iter().any(|current| current == entry.as_ref()))
                    || !editor
                        .read(cx)
                        .input_bounds()
                        .contains(&window.mouse_position())
                {
                    return;
                }
                this.diagnostic_hover = Some((index, entry.clone()));
                editor.update(cx, |state, cx| {
                    state.present_diagnostic((*entry).clone(), cx)
                });
            });
        }));
    }

    pub(super) fn show_first_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Prefer the current file; diagnostics can also point into an included file.
        let target = std::iter::once(self.active)
            .chain(0..self.documents.len())
            .find_map(|index| {
                let document = &self.documents[index];
                self.lsp
                    .diagnostics(&document.uri)
                    .into_iter()
                    .find(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
                    .map(|d| (document.uri.clone(), d.range))
            });
        if let Some((uri, range)) = target {
            let main = self.main_window;
            let workspace = cx.weak_entity();
            if main == window.window_handle() {
                if let Err(error) = self.navigate(&uri, Some(range), window, cx) {
                    self.lsp_error = Some(error.to_string());
                    cx.notify();
                }
            } else {
                cx.defer(move |cx| {
                    let _ = main.update(cx, |_, window, cx| {
                        let _ = workspace.update(cx, |this, cx| {
                            if let Err(error) = this.navigate(&uri, Some(range), window, cx) {
                                this.lsp_error = Some(error.to_string());
                                cx.notify();
                            }
                        });
                        window.activate_window();
                    });
                });
            }
        }
    }
    pub(super) fn inline_diagnostics(&self, _: &mut Context<Self>) -> impl IntoElement {
        let editor = self.documents[self.active].editor.clone();
        let font_size = px(f32::from(self.settings.font_size));
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let state = editor.read(cx);
                let Some(set) = state.diagnostics() else {
                    return;
                };
                let Some(visible) = state.visible_row_range() else {
                    return;
                };
                let Some(height) = state.line_height() else {
                    return;
                };
                let mut rows = std::collections::BTreeMap::new();
                for entry in set.iter() {
                    let severity = match entry.severity {
                        DiagnosticSeverity::Error => lsp_types::DiagnosticSeverity::ERROR,
                        DiagnosticSeverity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                        _ => continue,
                    };
                    let row = entry.diagnostic.range.start.line as usize;
                    if !visible.contains(&row) {
                        continue;
                    }
                    let item = rows
                        .entry(row)
                        .or_insert((severity, entry.message.clone(), 0usize));
                    if severity < item.0 {
                        item.0 = severity;
                        item.1 = entry.message.clone();
                    }
                    item.2 += 1;
                }
                let viewport = state.input_bounds();
                let rows: Vec<_> = rows
                    .into_iter()
                    .filter_map(|(row, (severity, message, count))| {
                        let end = state.text().line_end_offset(row);
                        let bounds = state.range_to_bounds(&(end..end))?;
                        // Native range geometry already includes both scroll axes.
                        Some((
                            bounds.origin + point(px(20.), px(0.)),
                            severity,
                            message,
                            count,
                        ))
                    })
                    .collect();
                window.with_content_mask(Some(ContentMask { bounds: viewport }), |window| {
                    for (origin, severity, message, count) in rows {
                        let available = viewport.right() - px(22.) - origin.x;
                        if available < px(40.)
                            || origin.y + height < viewport.top()
                            || origin.y > viewport.bottom()
                        {
                            continue;
                        }
                        let kind = if severity == lsp_types::DiagnosticSeverity::ERROR {
                            "Error"
                        } else {
                            "Warning"
                        };
                        let mut message = format!(
                            "{kind}: {}",
                            message.split_whitespace().collect::<Vec<_>>().join(" ")
                        );
                        if count > 1 {
                            message.push_str(&format!(" (+{})", count - 1));
                        }
                        let color = severity_color(severity);
                        let font = Font {
                            family: cx.theme().mono_font_family.clone(),
                            ..Default::default()
                        };
                        let shape = |text: String| {
                            window.text_system().shape_line(
                                text.clone().into(),
                                font_size,
                                &[TextRun {
                                    len: text.len(),
                                    font: font.clone(),
                                    color,
                                    ..Default::default()
                                }],
                                None,
                            )
                        };
                        let mut line = shape(message.clone());
                        if line.width() > available {
                            let mut characters: Vec<char> = message.chars().collect();
                            // Binary search the longest prefix plus ellipsis that fits.
                            let (mut low, mut high) = (0, characters.len());
                            while low < high {
                                let middle = (low + high).div_ceil(2);
                                let candidate = shape(format!(
                                    "{}…",
                                    characters[..middle].iter().collect::<String>()
                                ));
                                if candidate.width() <= available {
                                    low = middle;
                                } else {
                                    high = middle - 1;
                                }
                            }
                            characters.truncate(low);
                            line = shape(format!("{}…", characters.iter().collect::<String>()));
                        }
                        let background = Bounds::new(
                            origin - point(px(6.), px(0.)),
                            size(line.width() + px(12.), height),
                        );
                        window.paint_quad(fill(background, cx.theme().foreground.opacity(0.045)));
                        let _ = line.paint(origin, height, TextAlign::Left, None, window, cx);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    }
}
