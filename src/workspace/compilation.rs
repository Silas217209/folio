use super::*;
use gpui_kit::component::{WindowExt, text::TextView};

struct CompilationErrors {
    workspace: Entity<Workspace>,
    _subscription: Subscription,
}

impl Workspace {
    pub(super) fn compilation_details(&self) -> String {
        let mut sections = Vec::new();
        for (uri, diagnostic) in self.lsp.all_diagnostics() {
            if diagnostic.severity == Some(lsp_types::DiagnosticSeverity::HINT)
                || diagnostic.severity == Some(lsp_types::DiagnosticSeverity::INFORMATION)
            {
                continue;
            }
            let location = |uri: &str, range: lsp_types::Range| {
                let path = url::Url::parse(uri)
                    .ok()
                    .and_then(|url| url.to_file_path().ok());
                let path = path
                    .as_ref()
                    .map(|path| {
                        path.strip_prefix(&self.root)
                            .unwrap_or(path)
                            .display()
                            .to_string()
                    })
                    .unwrap_or_else(|| uri.to_owned());
                format!(
                    "{}:{}:{}",
                    path,
                    range.start.line + 1,
                    range.start.character + 1
                )
            };
            let kind = if diagnostic.severity == Some(lsp_types::DiagnosticSeverity::WARNING) {
                "Warning"
            } else {
                "Error"
            };
            let mut message = format!(
                "{kind}: {}\n{}",
                location(&uri, diagnostic.range),
                diagnostic.message
            );
            for related in diagnostic.related_information.unwrap_or_default() {
                message.push_str(&format!(
                    "\n{}\n{}",
                    location(related.location.uri.as_str(), related.location.range),
                    related.message
                ));
            }
            sections.push(message);
        }
        if let Some(error) = &self.compile_error {
            sections.push(error.clone());
        }
        if let Some(error) = &self.lsp_error {
            sections.push(format!("Language server: {error}"));
        }
        if sections.is_empty() {
            "No compilation errors.".into()
        } else {
            sections.join("\n\n")
        }
    }

    pub(super) fn show_compilation_errors(
        &mut self,
        _: &crate::ShowCompilationErrors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let workspace = cx.entity();
        let width = (window.viewport_size().width - px(48.)).min(px(760.));
        window.defer(cx, move |window, cx| {
            let content = cx.new(|cx| {
                let subscription = cx.observe(&workspace, |_, _, cx| cx.notify());
                CompilationErrors {
                    workspace: workspace.clone(),
                    _subscription: subscription,
                }
            });
            window.open_dialog(cx, move |dialog, _, _| {
                dialog
                    .title("Compilation errors")
                    .w(width)
                    .child(content.clone())
            });
        });
        cx.notify();
    }
}

impl Render for CompilationErrors {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let details = self.workspace.read(cx).compilation_details();
        // Treat compiler messages as literal text, including any backtick runs.
        let fence = "`".repeat(
            details
                .split(|ch| ch != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                .max(2)
                + 1,
        );
        let text = format!("{fence}\n{details}\n{fence}");
        let copy = self.workspace.clone();
        let source = self.workspace.clone();
        let has_source =
            self.workspace
                .read(cx)
                .lsp
                .all_diagnostics()
                .iter()
                .any(|(_, diagnostic)| {
                    diagnostic.severity == Some(lsp_types::DiagnosticSeverity::ERROR)
                });
        v_flex()
            .id("compile-errors")
            .test_support()
            .min_w_0()
            .gap_3()
            .child(
                div()
                    .id("compilation-error-scroll")
                    .test_support()
                    .min_w_0()
                    .max_h((window.viewport_size().height - px(220.)).clamp(px(80.), px(460.)))
                    .overflow_y_scroll()
                    .child(TextView::markdown("compilation-error-details", text).selectable(true)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("copy-compilation-errors")
                            .label("Copy details")
                            .on_click(move |_, _, cx| {
                                let text = copy.read(cx).compilation_details();
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }),
                    )
                    .child(
                        Button::new("compilation-error-source")
                            .label("Go to source")
                            .disabled(!has_source)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                source.update(cx, |workspace, cx| {
                                    workspace.show_first_error(window, cx)
                                });
                            }),
                    ),
            )
    }
}
