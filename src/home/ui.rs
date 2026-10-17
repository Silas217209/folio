use super::*;
use gpui_kit::component::{
    menu::{DropdownMenu, PopupMenuItem},
    tab::{Tab, TabBar},
};
pub(super) fn message(text: &str, error: bool) -> Div {
    h_flex()
        .mx_8()
        .mb_3()
        .px_4()
        .py_3()
        .rounded_md()
        .gap_3()
        .bg(if error { rgb(0xfff2ec) } else { rgb(0xedf6f2) })
        .text_size(px(12.))
        .text_color(if error { rgb(0x9a4c32) } else { rgb(0x2d6d5b) })
        .child(div().flex_1().min_w_0().child(text.to_owned()))
}
fn field(label: &str, id: &str, input: &Entity<InputState>) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_2()
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(0x69766e))
                .child(label.to_owned()),
        )
        .child(
            div()
                .id(SharedString::from(id.to_owned()))
                .test_support()
                .child(Input::new(input)),
        )
}
impl Home {
    pub(super) fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let root = self.library.root.clone();
        let templates = root.join("templates");
        let pdf = root.join("pdf");
        let has_contacts = invoice::contacts_path(&root).is_file();
        let owner = cx.weak_entity();
        h_flex()
            .px_8()
            .py_6()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(
                        root.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    ),
            )
            .child(
                Button::new("library-menu")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .tooltip(root.display().to_string())
                    .dropdown_menu(move |menu, _, _| {
                        let owner = owner.clone();
                        let root = root.clone();
                        let templates = templates.clone();
                        let pdf = pdf.clone();
                        menu.item(PopupMenuItem::new("Open library…").on_click(
                            move |_, window, cx| {
                                let _ =
                                    owner.update(cx, |this, cx| this.choose_library(window, cx));
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Show library folder")
                                .on_click(move |_, _, cx| cx.reveal_path(&root)),
                        )
                        .item(
                            PopupMenuItem::new("Open templates folder")
                                .on_click(move |_, _, cx| cx.open_with_system(&templates)),
                        )
                        .item(
                            PopupMenuItem::new("Open PDF folder")
                                .on_click(move |_, _, cx| cx.open_with_system(&pdf)),
                        )
                        .when(has_contacts, |menu| {
                            menu.separator()
                                .menu("Contacts…", Box::new(crate::OpenContacts))
                        })
                    }),
            )
            .child(
                Button::new("new-invoice")
                    .primary()
                    .icon(IconName::Plus)
                    .label("Create")
                    .disabled(self.templates.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.start_new(window, cx))),
            )
    }
    pub(super) fn documents(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut kinds: Vec<_> = self
            .templates
            .iter()
            .map(|t| t.name.clone())
            .chain(self.records.iter().map(|r| r.identity.kind.clone()))
            .collect();
        kinds.sort();
        kinds.dedup();
        let query = self.search.read(cx).value().to_lowercase();
        let visible: Vec<_> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                self.filter
                    .as_ref()
                    .is_none_or(|kind| &record.identity.kind == kind)
                    && (query.is_empty()
                        || record.identity.filename.to_lowercase().contains(&query))
            })
            .collect();
        let filters: Vec<_> = std::iter::once(None)
            .chain(kinds.into_iter().map(Some))
            .collect();
        let active = filters.iter().position(|f| *f == self.filter).unwrap_or(0);
        v_flex()
            .flex_1()
            .min_h_0()
            .px_8()
            .pb_8()
            .gap_4()
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        div().flex_1().min_w_0().child(
                            TabBar::new("document-types")
                                .selected_index(active)
                                .children(filters.into_iter().map(|kind| {
                                    let label = kind.clone().unwrap_or_else(|| "All".into());
                                    Tab::new().label(label).on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.filter = kind.clone();
                                            cx.notify();
                                        },
                                    ))
                                })),
                        ),
                    )
                    .child(div().w(px(250.)).child(
                        Input::new(&self.search).prefix(Icon::new(IconName::Search).size(px(15.))),
                    )),
            )
            .child(
                v_flex()
                    .id("document-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0xe3e8e3))
                    .bg(rgb(0xffffff))
                    .child(
                        h_flex()
                            .px_5()
                            .py_3()
                            .gap_3()
                            .text_size(px(12.))
                            .text_color(rgb(0x738078))
                            .border_b_1()
                            .border_color(rgb(0xe8ece7))
                            .child(div().flex_1().child("Name"))
                            .child(div().w(px(125.)).child("Type"))
                            .child(div().w(px(180.)).child("Last edited"))
                            .child(div().w(px(90.)).child("PDFs"))
                            .child(div().w(px(35.))),
                    )
                    .children(
                        visible
                            .iter()
                            .map(|(index, record)| self.document_row(*index, record, cx)),
                    )
                    .when(visible.is_empty(), |view| {
                        view.child(
                            v_flex()
                                .items_center()
                                .justify_center()
                                .py_16()
                                .gap_4()
                                .text_color(rgb(0x7c877f))
                                .child(Icon::new(IconName::FileText).size(px(28.)))
                                .child(if self.records.is_empty() {
                                    "No documents"
                                } else {
                                    "No matching documents"
                                }),
                        )
                    }),
            )
    }
    fn document_row(
        &self,
        index: usize,
        record: &Record,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let source = record.source.clone();
        let root = self.library.root.clone();
        let filename = record.entry(&root);
        let reveal = source.clone();
        let owner = cx.weak_entity();
        let count = self
            .export_counts
            .get(&record.identity.filename)
            .copied()
            .unwrap_or(0);
        let busy = self.busy.is_some();
        h_flex()
            .id(("document-row", index))
            .test_support()
            .px_5()
            .py_4()
            .gap_3()
            .text_size(px(13.))
            .border_b_1()
            .border_color(rgb(0xeff2ed))
            .child(
                div()
                    .id(("open-document", index))
                    .test_support()
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Icon::new(IconName::FileText)
                                    .size(px(18.))
                                    .text_color(rgb(0x4d8b79)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(record.stem()),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_document(&source, window, cx)
                    })),
            )
            .child(
                div()
                    .w(px(125.))
                    .truncate()
                    .text_color(rgb(0x778379))
                    .child(record.identity.kind.clone()),
            )
            .child(
                div()
                    .w(px(180.))
                    .text_color(rgb(0x778379))
                    .child(record.modified_label()),
            )
            .child(
                div().w(px(90.)).child(
                    Button::new(("pdf-history", index))
                        .ghost()
                        .small()
                        .label(if count == 0 {
                            "—".into()
                        } else {
                            format!("{count}")
                        })
                        .tooltip("PDF versions")
                        .on_click(move |_, window, cx| {
                            crate::pdf_history::open(root.clone(), filename.clone(), window, cx)
                        }),
                ),
            )
            .child(
                div().w(px(35.)).child(
                    Button::new(("document-menu", index))
                        .ghost()
                        .small()
                        .icon(IconName::Ellipsis)
                        .tooltip("Document actions")
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            for (label, intent) in [
                                ("Export PDF", Intent::Keep),
                                ("Email PDF…", Intent::Email),
                                ("Share PDF…", Intent::ChooseApp),
                            ] {
                                let owner = owner.clone();
                                let source = reveal.clone();
                                menu =
                                    menu.item(PopupMenuItem::new(label).disabled(busy).on_click(
                                        move |_, window, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                if let Some(index) = this
                                                    .records
                                                    .iter()
                                                    .position(|r| r.source == source)
                                                {
                                                    this.export(index, intent, window, cx);
                                                }
                                            });
                                        },
                                    ));
                            }
                            let reveal = reveal.clone();
                            menu.separator().item(
                                PopupMenuItem::new("Show source file")
                                    .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                            )
                        }),
                ),
            )
    }
    pub(super) fn create_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        let templates = self.templates.clone();
        let filename = self
            .selected
            .as_ref()
            .and_then(|template| {
                invoice::identity(
                    template,
                    &self.number.read(cx).value(),
                    &self.title.read(cx).value(),
                )
                .ok()
            })
            .map(|identity| identity.filename)
            .unwrap_or_else(|| {
                format!(
                    "{}_{}_Title.typ",
                    self.selected
                        .as_ref()
                        .map_or("Template", |t| t.name.as_str()),
                    self.number.read(cx).value()
                )
            });
        v_flex()
            .id("create-document-dialog")
            .test_support()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0x69766e))
                            .child("Template"),
                    )
                    .child(
                        Button::new("choose-template")
                            .w_full()
                            .label(
                                self.selected
                                    .as_ref()
                                    .map_or("Choose", |t| t.name.as_str())
                                    .to_owned(),
                            )
                            .icon(IconName::ChevronDown)
                            .dropdown_menu(move |menu, _, _| {
                                let mut menu = menu;
                                for template in &templates {
                                    let template = template.clone();
                                    let owner = owner.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(template.name.clone()).on_click(
                                            move |_, window, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.select_template(
                                                        template.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            },
                                        ),
                                    );
                                }
                                menu
                            }),
                    ),
            )
            .child(field("Number", "document-number", &self.number))
            .child(field("Title", "document-title", &self.title))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0x778379))
                    .truncate()
                    .child(filename),
            )
            .when_some(self.create_error.as_ref(), |view, error| {
                view.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xa04e36))
                        .child(error.clone()),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .mt_2()
                    .child(
                        Button::new("cancel-create")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("create-document")
                            .primary()
                            .label("Create")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.create_document(window, cx)),
                            ),
                    ),
            )
    }
}
