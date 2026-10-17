use crate::{
    invoice::{self, archive::ExportVersion},
    share::Intent,
};
use gpui_kit::{
    assets::IconName,
    base::TestSupportExt,
    component::{
        Icon, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    prelude::*,
    *,
};
use std::path::PathBuf;
struct PdfHistory {
    versions: Vec<ExportVersion>,
    error: Option<String>,
}
pub fn open(root: PathBuf, entry: String, window: &mut Window, cx: &mut App) {
    let (versions, error) = match invoice::archive::versions(&root, &entry) {
        Ok(v) => (v, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let view = cx.new(|_| PdfHistory { versions, error });
    window.defer(cx, move |window, cx| {
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("PDF versions").w(px(680.)).child(view.clone())
        })
    });
}
impl PdfHistory {
    fn handoff(&mut self, version: ExportVersion, intent: Intent, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    invoice::archive::verify(&version)?;
                    crate::share::handoff(&version.pdf, intent, &version.document.filename).await
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.error = result.err().map(|e| e.to_string());
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for PdfHistory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("pdf-versions")
            .test_support()
            .gap_3()
            .max_h(px(450.))
            .overflow_y_scroll()
            .when_some(self.error.as_ref(), |view, error| {
                view.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xa04e36))
                        .child(error.clone()),
                )
            })
            .when(self.versions.is_empty(), |view| {
                view.child(
                    div()
                        .py_8()
                        .text_color(rgb(0x778379))
                        .child("No PDF exports"),
                )
            })
            .children(self.versions.iter().map(|version| {
                let preview = version.clone();
                let reveal = version.pdf.clone();
                let email = version.clone();
                let share = version.clone();
                let date = chrono::DateTime::parse_from_rfc3339(&version.exported_at)
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%d %b %Y · %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|_| version.exported_at.clone());
                h_flex()
                    .gap_3()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(0xe5e9e7))
                    .child(Icon::new(IconName::FileText).size(px(18.)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div().truncate().text_size(px(13.)).child(format!(
                                    "v{} · {}",
                                    version.version, version.pdf_filename
                                )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x778379))
                                    .child(date),
                            ),
                    )
                    .child(
                        Button::new(("open-pdf-version", version.version as usize))
                            .ghost()
                            .small()
                            .label("Open")
                            .tooltip(if version.pdf_sha256.is_empty() {
                                "Imported PDF".into()
                            } else {
                                format!("SHA-256 {}", version.pdf_sha256)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match invoice::archive::verify(&preview) {
                                    Ok(()) => cx.open_with_system(&preview.pdf),
                                    Err(error) => {
                                        this.error = Some(error.to_string());
                                        cx.notify();
                                    }
                                }
                            })),
                    )
                    .child(
                        Button::new(("reveal-pdf-version", version.version as usize))
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .tooltip(crate::runtime::reveal_label())
                            // Revealing also helps locate an externally changed
                            // PDF, so do not require archive verification here.
                            .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                    )
                    .child(
                        Button::new(("email-pdf-version", version.version as usize))
                            .ghost()
                            .small()
                            .icon(IconName::Mail)
                            .tooltip("Email this version")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.handoff(email.clone(), Intent::Email, cx)
                            })),
                    )
                    .child(
                        Button::new(("share-pdf-version", version.version as usize))
                            .ghost()
                            .small()
                            .icon(IconName::Share2)
                            .tooltip("Share this version")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.handoff(share.clone(), Intent::ChooseApp, cx)
                            })),
                    )
            }))
    }
}
