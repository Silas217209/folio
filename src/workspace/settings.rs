use super::commands::{Command, RunCommand, SettingsPatch};
use super::*;
use crate::{OpenSettings, settings::Settings};
use gpui_kit::component::{
    WindowExt,
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
};

fn row(
    label: &'static str,
    description: &'static str,
    control: impl IntoElement,
) -> impl IntoElement {
    h_flex()
        .gap_4()
        .items_center()
        .py_2()
        .child(
            v_flex()
                .flex_1()
                .gap_1()
                .child(div().text_size(px(13.)).child(label))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(gpui_kit::rgb(0x77777c))
                        .child(description),
                ),
        )
        .child(control)
}
impl Workspace {
    pub(super) fn configure_document(
        document: &Document,
        settings: &Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        document.editor.update(cx, |state, cx| {
            state.set_soft_wrap(settings.soft_wrap, window, cx);
            state.set_line_number(settings.line_numbers, window, cx);
            state.set_tab_size(
                TabSize {
                    tab_size: settings.tab_width as usize,
                    hard_tabs: false,
                },
                cx,
            );
            state.set_readonly(
                settings.vim_enabled && document.vim.mode != vim::Mode::Insert,
                cx,
            );
        });
    }
    pub(super) fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = &self.settings_path {
            self.settings_error = self
                .settings
                .save(path)
                .err()
                .map(|error| format!("Could not save settings: {error}"));
        }
        cx.notify();
    }
    pub(super) fn update_settings(
        &mut self,
        update: impl FnOnce(&mut Settings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        update(&mut self.settings);
        for document in &self.documents {
            Self::configure_document(document, &self.settings, window, cx);
        }
        if !self.settings.follow_source {
            self.scroll_animation = None;
            self.follow_edit = None;
        }
        self.save_settings(cx);
        for index in 0..self.documents.len() {
            self.schedule_autosave(index, cx);
        }
    }
    pub(super) fn open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_open {
            return;
        }
        self.settings_open = true;
        let workspace = cx.entity();
        window.defer(cx, move |window, cx| {
            window.open_dialog(cx, move |dialog, _, cx| {
                let owner = workspace.downgrade();
                let content =
                    workspace.update(cx, |this, cx| this.settings_content(cx).into_any_element());
                dialog
                    .title("Settings")
                    .w(px(560.))
                    .child(content)
                    .on_close(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.settings_open = false;
                            cx.notify();
                        });
                    })
            });
        });
        cx.notify();
    }
    fn settings_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = &self.settings;
        let font_owner = cx.weak_entity();
        let font_size = settings.font_size;
        let tabs_owner = cx.weak_entity();
        let tab_width = settings.tab_width;
        v_flex()
            .id("settings-content")
            .test_support()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child("EDITOR"),
            )
            .child(row(
                "Autosave",
                "Save after a short pause in typing.",
                Switch::new("settings-autosave")
                    .small()
                    .checked(settings.autosave)
                    .accessibility_label("Autosave")
                    .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                        this.run_command(
                            &RunCommand::new(Command::Configure {
                                settings: SettingsPatch {
                                    autosave: Some(*enabled),
                                    ..Default::default()
                                },
                            }),
                            window,
                            cx,
                        );
                    })),
            ))
            .child(row(
                "Vim mode",
                "Modal editing with Normal, Insert, and Visual modes.",
                Switch::new("settings-vim")
                    .small()
                    .checked(settings.vim_enabled)
                    .accessibility_label("Vim mode")
                    .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                        this.run_command(
                            &RunCommand::new(Command::Configure {
                                settings: SettingsPatch {
                                    vim_enabled: Some(*enabled),
                                    ..Default::default()
                                },
                            }),
                            window,
                            cx,
                        );
                    })),
            ))
            .child(row(
                "Soft wrap",
                "Wrap long lines to the editor width.",
                Switch::new("settings-wrap")
                    .small()
                    .checked(settings.soft_wrap)
                    .accessibility_label("Soft wrap")
                    .on_change(cx.listener(|this, value: &bool, window, cx| {
                        this.run_command(
                            &RunCommand::new(Command::Configure {
                                settings: SettingsPatch {
                                    soft_wrap: Some(*value),
                                    ..Default::default()
                                },
                            }),
                            window,
                            cx,
                        )
                    })),
            ))
            .child(row(
                "Line numbers",
                "Show line numbers beside the source.",
                Switch::new("settings-lines")
                    .small()
                    .checked(settings.line_numbers)
                    .accessibility_label("Line numbers")
                    .on_change(cx.listener(|this, value: &bool, window, cx| {
                        this.run_command(
                            &RunCommand::new(Command::Configure {
                                settings: SettingsPatch {
                                    line_numbers: Some(*value),
                                    ..Default::default()
                                },
                            }),
                            window,
                            cx,
                        )
                    })),
            ))
            .child(row(
                "Font size",
                "Size of the source editor text.",
                Button::new("settings-font-size")
                    .small()
                    .label(format!("{font_size} px"))
                    .child(Icon::new(IconName::ChevronDown).size(px(12.)))
                    .dropdown_menu(move |mut menu, _, _| {
                        for size in [10, 11, 12, 13, 14, 16, 18, 20, 24, 28, 32] {
                            let owner = font_owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{size} px"))
                                    .checked(size == font_size)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.run_command(
                                                &RunCommand::new(Command::Configure {
                                                    settings: SettingsPatch {
                                                        font_size: Some(size),
                                                        ..Default::default()
                                                    },
                                                }),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            ))
            .child(row(
                "Indentation",
                "Insert spaces when pressing Tab.",
                Button::new("settings-tab-width")
                    .small()
                    .label(format!("{tab_width} spaces"))
                    .child(Icon::new(IconName::ChevronDown).size(px(12.)))
                    .dropdown_menu(move |mut menu, _, _| {
                        for width in [2, 4, 8] {
                            let owner = tabs_owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{width} spaces"))
                                    .checked(width == tab_width)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.run_command(
                                                &RunCommand::new(Command::Configure {
                                                    settings: SettingsPatch {
                                                        tab_width: Some(width),
                                                        ..Default::default()
                                                    },
                                                }),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            ))
            .child(
                div()
                    .mt_3()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child("PREVIEW"),
            )
            .child(row(
                "Follow edits",
                "Smoothly scroll to the page you are editing.",
                Switch::new("settings-follow")
                    .small()
                    .checked(settings.follow_source)
                    .accessibility_label("Follow edits")
                    .on_change(cx.listener(|this, value: &bool, window, cx| {
                        this.run_command(
                            &RunCommand::new(Command::Configure {
                                settings: SettingsPatch {
                                    follow_source: Some(*value),
                                    ..Default::default()
                                },
                            }),
                            window,
                            cx,
                        )
                    })),
            ))
            .when_some(self.settings_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .text_color(cx.theme().danger)
                        .text_size(px(12.))
                        .child(error.clone()),
                )
            })
    }
}
