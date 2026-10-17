use super::*;

use gpui_kit::InputEvent as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_paths(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/manuscript"),
            crate::settings::Settings {
                autosave: false,
                ..Default::default()
            },
            None,
            None,
            window,
            cx,
        )
    }
    pub(crate) fn with_paths(
        root: PathBuf,
        settings: crate::settings::Settings,
        settings_path: Option<PathBuf>,
        settings_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut project = crate::project::Project::load(&root).expect("valid test project");
        project.files.sort_by_key(|(path, _)| {
            crate::test_support::SAMPLE_FILES
                .iter()
                .position(|(sample, _)| path == sample)
                .unwrap_or(usize::MAX)
        });
        Self::with_project(project, settings, settings_path, settings_error, window, cx)
    }
}

fn choose_zoom(window: &mut Window, cx: &mut App, item: usize) {
    window.click("zoom-menu", cx);
    window.render_frame(cx);
    window.within("popup-menu").click(item, cx);
}

#[track_caller]
fn wait_language(
    app: &mut HeadlessAppContext,
    workspace: &Entity<Workspace>,
    condition: impl Fn(&Workspace, &App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.run_until_parked();
        if app.update(|cx| {
            let workspace = workspace.read(cx);
            assert!(workspace.lsp_error.is_none(), "{:?}", workspace.lsp_error);
            condition(workspace, cx)
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "{}", app.update(|cx| {
            let view = workspace.read(cx);
            let document = &view.documents[view.active];
            format!("Timed out waiting for editor LSP state; mode={:?} editable={} cursor={} menu_open={} source={:?}; server snapshot={:?}; diagnostics={:?}; editor diagnostics={:?}",
                document.vim.mode, document.editor.read(cx).is_editable(), document.editor.read(cx).cursor(), document.editor.read(cx).completion_menu_state().open,
                document.editor.read(cx).value(), view.lsp.snapshot(&document.uri).map(|snapshot| (snapshot.version, snapshot.text)),
                view.lsp.diagnostics(&document.uri), document.editor.read(cx).diagnostics())
        }));
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[::core::prelude::v1::test]
fn hover_popovers_wait_and_keep_source_geometry_fixed() {
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(800.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    let editor = app.update(|cx| workspace.read(cx).documents[0].editor.clone());
    let source = format!(
        "#let greeting = [Hello]\n{}\n#greeting",
        "Some wrapped source text. ".repeat(12)
    );
    app.update_window(handle, |_, window, cx| {
        editor.update(cx, |state, cx| state.set_value(source.clone(), window, cx));
        let view = workspace.read(cx);
        view.lsp.sync(&view.documents[0].uri, &source);
        window.render_frame(cx);
    })
    .unwrap();
    let geometry = |cx: &App| {
        let state = editor.read(cx);
        (
            state.input_bounds(),
            state.range_to_bounds(&(source.len() - 9..source.len())),
            state.scroll_offset(),
        )
    };
    let before = app.update(|cx| geometry(cx));
    app.update_window(handle, |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.present_hover(
                5..13,
                lsp_types::Hover {
                    contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                        kind: lsp_types::MarkupKind::Markdown,
                        value: "**greeting**\n\nDocumentation floats above the source.".into(),
                    }),
                    range: None,
                },
                cx,
            )
        });
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(editor.read(cx).hover_popover().is_some());
        assert_eq!(
            geometry(cx),
            before,
            "Opening hover must not resize or reflow source text"
        );
        editor.update(cx, |state, cx| state.clear_hover_state(cx));
        window.render_frame(cx);
        assert_eq!(
            geometry(cx),
            before,
            "Closing hover must not move source text"
        );
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        !view.lsp.capabilities().is_null()
    });
    let hover_position =
        app.update(|cx| editor.read(cx).range_to_bounds(&(5..13)).unwrap().center());
    let move_pointer = |window: &mut Window, position, cx: &mut App| {
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    };
    app.update_window(handle, |_, window, cx| {
        move_pointer(window, hover_position, cx)
    })
    .unwrap();
    app.run_until_parked();
    // Kit waits 150 ms before calling the provider; Folio waits another
    // 500 ms before sending the request. Passing over text stays quiet.
    app.advance_clock(Duration::from_millis(150));
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(499));
    app.run_until_parked();
    app.update(|cx| assert!(editor.read(cx).hover_popover().is_none()));
    app.advance_clock(Duration::from_millis(1));
    wait_language(&mut app, &workspace, |_, cx| {
        editor.read(cx).hover_popover().is_some()
    });
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(geometry(cx), before);
        move_pointer(window, point(px(2.), px(2.)), cx);
        move_pointer(window, hover_position, cx);
    })
    .unwrap();
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(150));
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        move_pointer(window, point(px(2.), px(2.)), cx);
    })
    .unwrap();
    app.advance_clock(Duration::from_secs(1));
    app.run_until_parked();
    app.update(|cx| {
        assert!(
            editor.read(cx).hover_popover().is_none(),
            "Leaving the source cancels pending hover"
        )
    });
    // Diagnostic tooltips use Kit's immediate mouse path rather than the
    // HoverProvider. They must follow the same delay and geometry rules.
    app.update_window(handle, |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.lsp_mut().hover_provider = None;
            let text = state.text().clone();
            let diagnostics = state.diagnostics_mut().unwrap();
            diagnostics.reset(&text);
            diagnostics.extend([lsp_types::Diagnostic::new_simple(
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 5),
                    lsp_types::Position::new(0, 13),
                ),
                "Example diagnostic details".into(),
            )]);
            cx.notify();
        });
        window.render_frame(cx);
        move_pointer(window, hover_position, cx);
        assert!(editor.read(cx).diagnostic_popover().is_none());
    })
    .unwrap();
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(649));
    app.run_until_parked();
    app.update(|cx| assert!(editor.read(cx).diagnostic_popover().is_none()));
    app.advance_clock(Duration::from_millis(1));
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(editor.read(cx).diagnostic_popover().is_some());
        assert_eq!(geometry(cx), before);
        move_pointer(window, hover_position, cx);
        assert!(editor.read(cx).diagnostic_popover().is_some());
        move_pointer(window, point(px(2.), px(2.)), cx);
        assert!(editor.read(cx).diagnostic_popover().is_none());
        move_pointer(window, hover_position, cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        editor.update(cx, |state, cx| {
            let text = state.text().clone();
            state.diagnostics_mut().unwrap().reset(&text);
            cx.notify();
        })
    });
    app.advance_clock(Duration::from_millis(650));
    app.run_until_parked();
    app.update(|cx| {
        assert!(
            editor.read(cx).diagnostic_popover().is_none(),
            "A cleared diagnostic must not reappear from a pending hover"
        )
    });
}

#[::core::prelude::v1::test]
fn editor_lsp_completion_navigation_rename_diagnostics_and_formatting() {
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
        crate::bind_editor_keys(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(940.)),
                ))),
                show: false,
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    wait_language(&mut app, &workspace, |view, _| {
        !view.lsp.capabilities().is_null()
    });
    app.update_window(handle, |_, window, cx| {
        let editor = workspace.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| {
            state.set_value("#let number = 42\n#te", window, cx);
            state.set_cursor_position(lsp_types::Position::new(1, 3), window, cx);
        });
        window.render_frame(cx);
        window.input("x", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .completion_menu_state()
            .open
    });
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    if let Ok(path) = std::env::var("TYPST_EDITOR_LSP_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        window.press("escape", cx);
        window.press("ctrl-space", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .completion_menu_state()
            .open
    });
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0].editor.read(cx).value().contains("#text")
    });
    app.update(|cx| {
        let value = workspace.read(cx).documents[0].editor.read(cx).value();
        assert!(
            !value.contains("${"),
            "Completion inserted a literal snippet: {value}"
        );
    });

    let main = "#import \"notes.typ\": greet\n#let marker = \"😀\"\n#greet(\"Ada\")\n";
    let notes = "#let greet(name) = [Hello #name]\n";
    app.update_window(handle, |_, window, cx| {
        workspace.update(cx, |view, cx| {
            for (index, source) in [(0, main), (3, notes)] {
                view.documents[index]
                    .editor
                    .update(cx, |state, cx| state.set_value(source, window, cx));
                view.lsp.sync(&view.documents[index].uri, source);
                view.service.update(
                    view.documents[index].path.clone().into(),
                    source.into(),
                    None,
                );
            }
            view.documents[0].editor.update(cx, |state, cx| {
                state.set_cursor_position(lsp_types::Position::new(2, 3), window, cx)
            });
        });
        window.render_frame(cx);
        window.press("f12", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| view.active == 3);
    app.update_window(handle, |_, window, cx| {
        let state = workspace.read(cx).documents[3].editor.read(cx);
        assert_eq!(&state.value()[state.selected_range()], "greet");
        window.render_frame(cx);
        window.press("shift-f12", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        view.references
            .as_ref()
            .is_some_and(|references| references.len() >= 2)
    });
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("close-references", cx);
        window.press("f2", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| view.rename_target.is_some());
    app.update_window(handle, |_, window, cx| {
        workspace
            .read(cx)
            .rename_input
            .clone()
            .update(cx, |state, cx| state.set_value("salute", window, cx));
        window.render_frame(cx);
        window.click("confirm-rename", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .value()
            .contains("#salute")
            && view.documents[3]
                .editor
                .read(cx)
                .value()
                .contains("#let salute")
    });
    app.update_window(handle, |_, window, cx| {
        let view = workspace.read(cx);
        assert!(view.documents[0].dirty && view.documents[3].dirty);
        assert!(
            !fs::read_to_string(view.root.join("notes.typ"))
                .unwrap()
                .contains("#let salute")
        );
        workspace.update(cx, |view, cx| {
            view.open_document(0, window, cx);
            view.documents[0].editor.update(cx, |state, cx| {
                state.set_value("#let   value= (1,2)\n#value", window, cx);
                state.set_cursor_position(lsp_types::Position::new(1, 6), window, cx);
            });
        });
        window.render_frame(cx);
        window.press("ctrl-shift-i", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .value()
            .contains("#let value =")
    });
    app.update_window(handle, |_, window, cx| {
        window.press("ctrl-z", cx);
        assert_eq!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .value()
                .as_ref(),
            "#let   value= (1,2)\n#value"
        );
        window.press("ctrl-shift-z", cx);
        assert!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .value()
                .contains("#let value =")
        );
        let editor = workspace.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| {
            let source = state.value().to_string();
            state.set_selected_range(source.len()..source.len(), cx);
            state.focus(window, cx);
        });
        window.render_frame(cx);
        window.input("\n#missing_symbol_for_diagnostic", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .diagnostics()
            .is_some_and(|set| {
                set.iter()
                    .any(|diagnostic| diagnostic.message.contains("missing_symbol_for_diagnostic"))
            })
    });
    app.update_window(handle, |_, window, cx| {
        let editor = workspace.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| {
            let source = state.value();
            let start = source.find("\n#missing_symbol_for_diagnostic").unwrap();
            state.set_selected_range(start..source.len(), cx);
        });
        window.render_frame(cx);
        window.input("\nAll good", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .diagnostics()
            .is_some_and(|set| set.is_empty())
    });
    app.update_window(handle, |_, window, cx| {
        workspace.read(cx).documents[0]
            .editor
            .clone()
            .update(cx, |state, cx| {
                state.set_value("#text(size: 12pt)[Hello]", window, cx);
                state.set_cursor_position(lsp_types::Position::new(0, 10), window, cx);
            });
        window.render_frame(cx);
        window.press("ctrl-shift-space", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        view.signature
            .as_ref()
            .is_some_and(|signature| signature.contains("text("))
    });
    app.update_window(handle, |_, window, cx| {
        workspace.read(cx).documents[0]
            .editor
            .clone()
            .update(cx, |state, cx| {
                state.set_value("$x + y$", window, cx);
                state.set_cursor_position(lsp_types::Position::new(0, 3), window, cx);
            });
        window.render_frame(cx);
        window.press("ctrl-.", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .code_action_menu_state()
            .items
            .iter()
            .any(|item| item.action.title == "Convert to block equation")
    });
    app.update_window(handle, |_, window, cx| {
        let index = workspace.read(cx).documents[0]
            .editor
            .read(cx)
            .code_action_menu_state()
            .items
            .iter()
            .position(|item| item.action.title == "Convert to block equation")
            .unwrap();
        window.render_frame(cx);
        for _ in 0..index {
            window.press("down", cx);
        }
        window.press("enter", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0].editor.read(cx).value().as_ref() == "$ x + y $"
    });
}

#[::core::prelude::v1::test]
fn workspace_closes_and_reopens_tabs_without_losing_edits() {
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(940.)),
                ))),
                show: false,
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.run_until_parked();
        let ready = app.update(|cx| {
            let view = workspace.read(cx);
            assert!(view.compile_error.is_none(), "{:?}", view.compile_error);
            view.preview.is_some()
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "preview did not arrive");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.update_window(handle, |_, window, cx| {
        workspace.update(cx, |view, cx| view.open_document(1, window, cx));
        window.render_frame(cx);
        window.within("source-tabs").click(0usize, cx);
        assert_eq!(workspace.read(cx).active, 0);
        window.within("source-tabs").click(1usize, cx);
        assert_eq!(workspace.read(cx).active, 1);
        window.input("// unsaved\n", cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        assert!(workspace.read(cx).documents[1].dirty);
        let text = workspace.read(cx).documents[1].editor.read(cx).value();
        window.render_frame(cx);
        window.click(("close-tab", 1usize), cx);
        assert_eq!(workspace.read(cx).tabs, vec![0]);
        assert_eq!(workspace.read(cx).active, 0);
        workspace.update(cx, |view, cx| view.open_document(1, window, cx));
        assert_eq!(
            workspace.read(cx).documents[1].editor.read(cx).value(),
            text
        );
        workspace.update(cx, |view, cx| view.close_tab(1, window, cx));
        window.render_frame(cx);
        window.click(("close-tab", 0usize), cx);
        assert!(workspace.read(cx).tabs.is_empty());
        window.render_frame(cx);
        workspace.update(cx, |view, cx| view.open_document(0, window, cx));
        window.render_frame(cx);
        window.click("zoom-in", cx);
        assert_eq!(workspace.read(cx).zoom, 1.25);
        choose_zoom(window, cx, 9);
        window.click("toggle-sidebar", cx);
        assert!(!workspace.read(cx).sidebar_visible);
        window.click("toggle-sidebar", cx);
        window.render_frame(cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| assert_eq!(workspace.read(cx).zoom, 1.));
    if let Ok(path) = std::env::var("TYPST_EDITOR_SCREENSHOT") {
        app.run_until_parked();
        app.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        app.capture_screenshot(handle).unwrap().save(&path).unwrap();
        app.update_window(handle, |_, window, cx| {
            workspace.update(cx, |view, cx| {
                view.set_zoom(8., None, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        app.capture_screenshot(handle)
            .unwrap()
            .save(path.replace(".png", "-zoom.png"))
            .unwrap();
    }

    if let Ok(path) = std::env::var("FOLIO_HEADINGS_SCREENSHOT") {
        let old_page = app.update(|cx| workspace.read(cx).preview.as_ref().unwrap().pages[0].id);
        app.update_window(handle, |_, window, cx| {
            let editor = workspace.read(cx).documents[0].editor.clone();
            editor.update(cx, |state, cx| {
                state.replace_all("= Introduction\n#lorem(60)\n\n== In this paper\n#lorem(20)\n\n=== Contributions\n#lorem(40)\n\n= Related Work\n#lorem(50)\n\nHello world\n", window, cx);
                state.set_cursor_position(lsp_types::Position::new(13,0), window, cx);
            });
        }).unwrap();
        wait_language(&mut app, &workspace, |view, _| {
            !view.compiling
                && view
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.pages[0].id != old_page)
        });
        settle_surface(&mut app, handle);
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }

    // Exercise the full editor -> compiler -> source map -> smooth scroll
    // path with three pages. The fixture stays in memory, never on disk.
    app.update_window(handle, |_, window, cx| {
        let editor = workspace.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| {
            state.set_value("#set page(width: 500pt, height: 700pt, margin: 40pt)\nFirst page\n#pagebreak()\nSecond page\n#pagebreak()\nThird page", window, cx);
            state.set_cursor_position(gpui_kit::base::input::Position::new(5, 5), window, cx);
        });
        workspace.update(cx, |view, cx| {
            view.set_zoom(1., None, cx);
            view.preview_scroll.set_offset(Point::default());
        });
        window.render_frame(cx);
        window.input(" edited", cx);
    }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.run_until_parked();
        let ready = app.update(|cx| {
            let view = workspace.read(cx);
            assert!(view.compile_error.is_none(), "{:?}", view.compile_error);
            view.preview
                .as_ref()
                .is_some_and(|preview| preview.pages.len() == 3)
                && view.scroll_animation.is_some()
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "source-follow did not arrive");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let first = window.find(("preview-page", 0usize)).bounds();
        let second = window.find(("preview-page", 1usize)).bounds();
        let third = window.find(("preview-page", 2usize)).bounds();
        assert_eq!(first.size, size(px(500.), px(700.)));
        assert_eq!(second.top() - first.bottom(), px(24.));
        assert_eq!(third.top() - second.bottom(), px(24.));
        assert_eq!(workspace.read(cx).preview_scroll.offset().y, px(0.));
    })
    .unwrap();
    app.advance_clock(Duration::from_millis(16));
    app.run_until_parked();
    let intermediate = app.update(|cx| workspace.read(cx).preview_scroll.offset().y);
    assert!(intermediate < px(0.) && intermediate > px(-1000.));
    for _ in 1..14 {
        app.advance_clock(Duration::from_millis(16));
        app.run_until_parked();
        app.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        let view = workspace.read(cx);
        let offset = view.preview_scroll.offset();
        assert!(offset.y < px(-1000.));
        assert!(offset.y < intermediate);
        assert!(window.find(("preview-page", 2usize)).visible());
        let revision = view.edit_revision;
        workspace.update(cx, |view, cx| {
            view.follow_location(
                PreviewLocation {
                    revision: revision - 1,
                    page: 0,
                    x: 40.,
                    y: 40.,
                },
                cx,
            );
            assert_eq!(view.preview_scroll.offset(), offset);
        });
        window.scroll(
            "preview-scroll",
            ScrollDelta::Pixels(point(px(0.), px(200.))),
            cx,
        );
        assert!(workspace.read(cx).scroll_animation.is_none());
        assert!(workspace.read(cx).follow_edit.is_none());
    })
    .unwrap();
    // A delayed LSP edit must restore a valid active tab when it reopens a
    // buffer after the user closes every visible tab. No files are saved.
    app.update_window(handle, |_, window, cx| {
        workspace.update(cx, |view, cx| {
            view.open_document(1, window, cx);
            view.close_tab(0, window, cx);
            view.close_tab(1, window, cx);
            assert!(view.tabs.is_empty());
            assert_eq!(view.active, 1);
            let edit = serde_json::from_value(serde_json::json!({
                "changes": { view.documents[0].uri.clone(): [{
                    "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":0}},
                    "newText":"// late edit\n"
                }] }
            }))
            .unwrap();
            view.apply_workspace_edit(edit, None, window, cx).unwrap();
            assert_eq!(view.active, 0);
            assert_eq!(view.tabs, vec![0]);
            assert!(
                view.documents[0]
                    .editor
                    .read(cx)
                    .value()
                    .starts_with("// late edit\n")
            );
        });
    })
    .unwrap();
}
#[::core::prelude::v1::test]
fn vim_native_commands_completion_preview_and_focus() {
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
        crate::bind_editor_keys(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(940.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    wait_language(&mut app, &workspace, |view, _| {
        !view.lsp.capabilities().is_null()
    });
    app.update_window(handle, |_, window, cx| {
        workspace.read(cx).documents[0]
            .editor
            .clone()
            .update(cx, |state, cx| {
                state.set_value("one elephant three\nsecond line\n", window, cx);
                state.set_cursor_position(lsp_types::Position::new(0, 0), window, cx);
                state.focus(window, cx);
            });
        window.render_frame(cx);
        window.press("f6", cx);
        window.render_frame(cx);
        assert!(workspace.read(cx).settings.vim_enabled);
        // Normal-mode printable keys are commands, never inserted as text.
        window.input("zzzz", cx);
        for key in ["c", "i", "w"] {
            window.press(key, cx);
        }
        assert_eq!(workspace.read(cx).documents[0].vim.mode, vim::Mode::Insert);
        window.input("small", cx);
        window.press("escape", cx);
        window.press("w", cx);
        window.press(".", cx);
        assert_eq!(
            workspace.read(cx).documents[0].editor.read(cx).value(),
            "small small three\nsecond line\n"
        );
        window.press("u", cx);
        assert_eq!(
            workspace.read(cx).documents[0].editor.read(cx).value(),
            "small elephant three\nsecond line\n"
        );
        window.press("u", cx);
        assert_eq!(
            workspace.read(cx).documents[0].editor.read(cx).value(),
            "one elephant three\nsecond line\n"
        );
        window.press("ctrl-r", cx);
        window.press("ctrl-r", cx);
        // A named register crosses buffers; closing/reopening preserves mode/history.
        for key in ["g", "g", "\"", "a", "y", "y"] {
            window.press(key, cx);
        }
        workspace.update(cx, |view, cx| view.open_document(3, window, cx));
        window.render_frame(cx);
        for key in ["\"", "a", "p"] {
            window.press(key, cx);
        }
        assert!(
            workspace.read(cx).documents[3]
                .editor
                .read(cx)
                .value()
                .contains("small small three")
        );
        window.press("u", cx);
        workspace.update(cx, |view, cx| view.open_document(0, window, cx));
        window.render_frame(cx);
        // Search input receives ordinary typing, and Escape restores editor focus.
        window.press("/", cx);
        window.render_frame(cx);
        window.input("second", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(workspace.read(cx).vim_command.is_none());
        let state = workspace.read(cx).documents[0].editor.read(cx);
        assert_eq!(state.cursor_position().line, 1);
        assert!(state.focus_handle(cx).is_focused(window));
        window.press(":", cx);
        window.render_frame(cx);
        window.input("1", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert_eq!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .cursor_position()
                .line,
            0
        );
        window.press(":", cx);
        window.render_frame(cx);
        window.input("unused", cx);
        window.press("escape", cx);
        assert!(workspace.read(cx).vim_command.is_none());
        window.render_frame(cx);
    })
    .unwrap();
    // Focus/blur notifications from the command bar run at the end of an
    // event-loop turn, before a user starts the next editing gesture.
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        // Completion still uses the native menu and Tinymist in Insert mode.
        workspace.read(cx).documents[0]
            .editor
            .clone()
            .update(cx, |state, cx| {
                state.set_value("#let number = 42\n#te", window, cx);
                state.set_cursor_position(lsp_types::Position::new(1, 2), window, cx);
            });
        window.press("shift-a", cx);
        window.render_frame(cx);
        window.input("x", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[0]
            .editor
            .read(cx)
            .completion_menu_state()
            .open
    });
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    // Kit accepts the completion on its UI executor. Let that event-loop
    // turn finish before the next key, as it does for native OS events.
    wait_language(&mut app, &workspace, |view, cx| {
        let document = &view.documents[0];
        let source = document.editor.read(cx).value();
        document.dirty
            && source.contains("#text")
            && view
                .lsp
                .snapshot(&document.uri)
                .is_some_and(|snapshot| snapshot.text == source.as_str())
    });
    app.update_window(handle, |_, window, cx| {
        window.press("escape", cx);
        for key in ["shift-f", "(", "a"] {
            window.press(key, cx);
        }
        window.input("\"Vim preview\"", cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .value()
                .contains("#text(\"Vim preview\")")
        );
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        !view.compiling && view.compile_error.is_none() && view.preview.is_some()
    });
    if let Ok(path) = std::env::var("TYPST_EDITOR_VIM_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    // Vim's language commands use the same real Tinymist providers while
    // the source editor rejects ordinary text input in Normal mode.
    app.update_window(handle, |_, window, cx| {
        workspace.update(cx, |view, cx| {
            for (index, source) in [
                (0, "#import \"notes.typ\": greet\n#greet(\"Ada\")\n"),
                (3, "#let greet(name) = [Hello #name]\n"),
            ] {
                view.documents[index]
                    .editor
                    .update(cx, |state, cx| state.set_value(source, window, cx));
                view.lsp.sync(&view.documents[index].uri, source);
            }
            view.documents[0].editor.update(cx, |state, cx| {
                state.set_cursor_position(lsp_types::Position::new(1, 3), window, cx)
            });
        });
        window.render_frame(cx);
        window.press("g", cx);
        window.press("d", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| view.active == 3);
    app.update_window(handle, |_, window, cx| {
        assert!(
            workspace.read(cx).documents[3]
                .editor
                .read(cx)
                .selected_range()
                .is_empty()
        );
        window.press("shift-k", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        view.documents[3].editor.read(cx).hover_popover().is_some()
    });
    app.update_window(handle, |_, window, cx| {
        window.press("escape", cx);
        window.press("g", cx);
        window.press("r", cx);
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        view.references
            .as_ref()
            .is_some_and(|references| references.len() >= 2)
    });
    app.update_window(handle, |_, window, cx| {
        window.press("escape", cx);
        assert!(workspace.read(cx).references.is_none());
        workspace.update(cx, |view, cx| view.open_document(0, window, cx));
    })
    .unwrap();
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).documents[0].vim.mode, vim::Mode::Normal);
        window.press("f6", cx);
        window.render_frame(cx);
        assert!(!workspace.read(cx).settings.vim_enabled);
        window.input("ordinary", cx);
        assert!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .value()
                .contains("ordinary")
        );
        assert!(workspace.read(cx).follow_edit.is_some());
    })
    .unwrap();
}
fn settle_surface(app: &mut HeadlessAppContext, handle: AnyWindowHandle) {
    for _ in 0..3 {
        app.run_until_parked();
        app.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}
#[::core::prelude::v1::test]
fn source_diagnostics_mark_closed_files_and_clear_after_recovery() {
    use gpui_kit::component::input::RopeExt;
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
        crate::bind_editor_keys(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(940.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    wait_language(&mut app, &workspace, |view, _| view.preview.is_some());
    app.update_window(handle, |_, window, cx| {
        let child = workspace.read(cx).documents[2].editor.clone();
        child.update(cx, |state, cx| {
            state.replace_all("Included file is valid.", window, cx)
        });
        let main = workspace.read(cx).documents[0].editor.clone();
        main.update(cx, |state, cx| {
            state.replace_all(
                "#set page(width: 420pt, height: 600pt)\n= Diagnostic demo\n\n#include \"chapters/model.typ\"",
                window,
                cx,
            )
        });
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        !view.compiling
            && view.compile_error.is_none()
            // Status and vector frames arrive on separate sockets. Wait for
            // the replacement document, not a status for the previous frame.
            && view.preview.as_ref().is_some_and(|preview| preview.pages[0].width() == 420.)
            && view.documents[0]
                .editor
                .read(cx)
                .diagnostics()
                .is_some_and(|set| set.is_empty())
            && view.documents[2]
                .editor
                .read(cx)
                .diagnostics()
                .is_some_and(|set| set.is_empty())
    });
    let page = app.update(|cx| workspace.read(cx).preview.as_ref().unwrap().pages[0].id);
    app.update_window(handle, |_, window, cx| {
        let child = workspace.read(cx).documents[2].editor.clone();
        child.update(cx, |state, cx| {
            state.replace_all(
                format!("{}é 😀 #missing_included_value", "\n".repeat(45)),
                window,
                cx,
            )
        });
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        view.compile_error.is_some()
            && diagnostics::file_severity(&view.lsp.diagnostics(&view.documents[2].uri))
                == Some(lsp_types::DiagnosticSeverity::ERROR)
    });
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(!workspace.read(cx).tabs.contains(&2));
        assert_eq!(
            workspace.read(cx).preview.as_ref().unwrap().pages[0].id,
            page
        );
        assert!(window.find("preview-warning").visible());
        assert!(window.try_find("compile-errors").is_none());
        assert!(window.try_find("soft-wrap").is_none());
        window.click("file-diagnostic:chapters/model.typ", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| window.click("preview-warning", cx))
        .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(window.find("compile-errors").visible());
        window.click("compilation-error-source", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    let (message_origin, viewport, window_size) = app
        .update_window(handle, |_, window, cx| {
            let view = workspace.read(cx);
            assert_eq!(view.active, 2);
            assert!(window.find("inline-diagnostics").visible());
            let state = view.documents[2].editor.read(cx);
            let end = state.text().line_end_offset(45);
            let bounds = state.range_to_bounds(&(end..end)).unwrap();
            let geometry = state.prepaint_caret_geometry().unwrap();
            let caret = geometry.cursor_bounds();
            let expected_y = caret.top() + geometry.scroll_offset().y
                - (geometry.line_height() - caret.size.height) / 2.;
            assert!(
                (f32::from(bounds.top() - expected_y)).abs() < 0.1,
                "Diagnostic should be on the caret's source line"
            );
            assert!(
                (f32::from(bounds.left() - caret.left())).abs() < 1.1,
                "Diagnostic should start after the last source glyph"
            );
            (
                bounds.origin + point(px(20.), px(0.)),
                state.input_bounds(),
                window.viewport_size(),
            )
        })
        .unwrap();
    let image = app.capture_screenshot(handle).unwrap();
    let scale = image.width() as f32 / f32::from(window_size.width);
    let label = image::imageops::crop_imm(
        &image,
        (f32::from(message_origin.x) * scale).ceil() as u32,
        (f32::from(message_origin.y) * scale).ceil() as u32,
        (f32::from(viewport.right() - message_origin.x - px(16.)) * scale).floor() as u32,
        (20. * scale) as u32,
    )
    .to_image();
    assert!(
        label
            .pixels()
            .filter(|p| p[0] > 120 && p[1] < 110 && p[2] < 110)
            .count()
            > 20,
        "Expected a red diagnostic message after the source line"
    );
    if let Ok(path) = std::env::var("FOLIO_DIAGNOSTICS_SCREENSHOT") {
        image.save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        window.click("preview-warning", cx);
        let child = workspace.read(cx).documents[2].editor.clone();
        assert_eq!(child.read(cx).cursor_position().line, 45);
        window.close_dialog(cx);
        child.update(cx, |state, cx| {
            state.replace_all(
                "#set text(font: \"folio-unavailable-font-for-test\")\nWarning example.",
                window,
                cx,
            )
        });
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        !view.compiling
            && view.compile_error.is_none()
            && diagnostics::file_severity(&view.lsp.diagnostics(&view.documents[2].uri))
                == Some(lsp_types::DiagnosticSeverity::WARNING)
    });
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(window.try_find("preview-warning").is_none());
        assert!(window.find("file-diagnostic:chapters/model.typ").visible());
        assert!(window.find("inline-diagnostics").visible());
        let child = workspace.read(cx).documents[2].editor.clone();
        child.update(cx, |state, cx| state.replace_all("All good.", window, cx));
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, cx| {
        !view.compiling
            && view.compile_error.is_none()
            && view.lsp.diagnostics(&view.documents[2].uri).is_empty()
            && view.documents[2]
                .editor
                .read(cx)
                .diagnostics()
                .is_some_and(|set| set.is_empty())
    });
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, _| {
        assert!(window.try_find("preview-warning").is_none());
        assert!(
            window
                .try_find("file-diagnostic:chapters/model.typ")
                .is_none()
        );
        assert!(window.try_find("inline-diagnostics").is_none());
    })
    .unwrap();
}
#[::core::prelude::v1::test]
fn settings_persist_and_modal_cursor_stays_single_after_scrolling() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
        crate::bind_editor_keys(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(940.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        workspace.update(cx, |this, cx| {
            this.settings_path = Some(path.clone());
            this.documents[0].editor.update(cx, |state, cx| {
                state.set_value(vec![" ".repeat(200); 80].join("\n"), window, cx);
                state.focus(window, cx);
            });
        });
        window.press("ctrl-,", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    for control in [
        "settings-vim",
        "settings-wrap",
        "settings-lines",
        "settings-follow",
    ] {
        app.update_window(handle, |_, window, cx| {
            assert!(window.find("settings-content").visible());
            assert!(window.try_find("settings-done").is_none());
            window.click(control, cx);
        })
        .unwrap();
        settle_surface(&mut app, handle);
    }
    app.update_window(handle, |_, window, cx| {
        window.click("settings-font-size", cx);
        window.render_frame(cx);
        window.within("popup-menu").click(6usize, cx); // 18 px.
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        window.click("settings-tab-width", cx);
        window.render_frame(cx);
        window.within("popup-menu").click(1usize, cx); // Four spaces.
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update(|cx| {
        let view = workspace.read(cx);
        assert_eq!(view.settings.font_size, 18);
        assert_eq!(view.settings.tab_width, 4);
        assert!(view.settings.vim_enabled);
        assert!(
            !view.settings.soft_wrap && !view.settings.line_numbers && !view.settings.follow_source,
            "Settings switches did not apply: {:?}",
            view.settings
        );
        assert_eq!(
            crate::settings::Settings::load(&path).unwrap(),
            view.settings
        );
    });
    app.advance_clock(Duration::from_millis(300));
    settle_surface(&mut app, handle);
    if let Ok(path) = std::env::var("FOLIO_SETTINGS_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        window.within("dialog").click("close", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        for key in ["4", "0", "shift-g", "0", "1", "5", "0", "l"] {
            window.press(key, cx);
        }
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        for key in ["2", "0", "h", "1", "0", "k"] {
            window.press(key, cx);
        }
    })
    .unwrap();
    settle_surface(&mut app, handle);
    let (bounds, viewport) = app
        .update_window(handle, |_, window, cx| {
            let view = workspace.read(cx);
            let state = view.documents[0].editor.read(cx);
            assert!(state.scroll_offset().y < px(-100.));
            assert!(state.scroll_offset().x < px(-100.));
            assert_eq!(state.cursor_position(), lsp_types::Position::new(29, 130));
            assert_eq!(state.line_height().unwrap(), px(27.));
            (state.input_bounds(), window.viewport_size())
        })
        .unwrap();
    let mut frames = Vec::new();
    for delay in [350, 500, 500] {
        app.advance_clock(Duration::from_millis(delay));
        settle_surface(&mut app, handle);
        let image = app.capture_screenshot(handle).unwrap();
        let scale = image.width() as f32 / f32::from(viewport.width);
        frames.push(
            image::imageops::crop_imm(
                &image,
                (f32::from(bounds.left() + px(4.)) * scale).ceil() as u32,
                (f32::from(bounds.top() + px(4.)) * scale).ceil() as u32,
                (f32::from(bounds.size.width - px(28.)) * scale).floor() as u32,
                (f32::from(bounds.size.height - px(28.)) * scale).floor() as u32,
            )
            .to_image(),
        );
    }
    assert_eq!(
        frames[0], frames[1],
        "A second caret blinked in a scrolled editor"
    );
    assert_eq!(frames[1], frames[2]);
    let ink: Vec<_> = frames[0]
        .enumerate_pixels()
        .filter(|(_, _, p)| p[0] < 100 && (100..170).contains(&p[1]) && p[2] > 180)
        .map(|(x, y, _)| (x, y))
        .collect();
    assert!(!ink.is_empty(), "The block must follow the scrolled caret");
    let width = ink.iter().map(|p| p.0).max().unwrap() - ink.iter().map(|p| p.0).min().unwrap() + 1;
    let height =
        ink.iter().map(|p| p.1).max().unwrap() - ink.iter().map(|p| p.1).min().unwrap() + 1;
    assert!(
        (8..=24).contains(&width) && height >= 25,
        "Expected a single block, found {width} by {height}"
    );
    assert!(
        ink.len() as f32 > (width * height) as f32 * 0.9,
        "Expected one solid rectangle, with no extra thin caret"
    );
    if let Ok(path) = std::env::var("FOLIO_CURSOR_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        let editor = workspace.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| {
            state.set_cursor_position(lsp_types::Position::new(29, 200), window, cx)
        });
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert_eq!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .cursor_position(),
            lsp_types::Position::new(29, 199)
        );
        window.press("i", cx);
        assert_eq!(workspace.read(cx).documents[0].vim.mode, vim::Mode::Insert);
        assert!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .is_editable()
        );
        window.press("ctrl-,", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.advance_clock(Duration::from_millis(300));
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        window.click("settings-vim", cx);
    })
    .unwrap();
    // Deliver the switch change before closing its dialog, as separate
    // native pointer events do in the application.
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        window.within("dialog").click("close", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(!workspace.read(cx).settings.vim_enabled);
        assert!(window.try_find("vim-mode").is_none());
        assert!(!workspace.read(cx).settings_open);
    })
    .unwrap();
}
#[::core::prelude::v1::test]
fn status_cursor_fit_resize_and_live_detached_preview() {
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
        crate::bind_editor_keys(cx);
    });
    let (handle, workspace) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(940.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    });
    wait_language(&mut app, &workspace, |view, _| view.preview.is_some());
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        workspace.read(cx).documents[0]
            .editor
            .clone()
            .update(cx, |state, cx| {
                state.set_value("alpha β\nsecond line\nthird line\n", window, cx);
                state.set_cursor_position(lsp_types::Position::new(0, 0), window, cx);
                state.focus(window, cx);
            });
        window.render_frame(cx);
        assert!(window.try_find("vim-mode").is_none());
        let sidebar_toggle = window.find("toggle-sidebar").bounds();
        assert!(sidebar_toggle.left() < px(40.));
        assert!(sidebar_toggle.top() > window.viewport_size().height - px(30.));
        let settings_button = window.find("open-settings").bounds();
        assert!(settings_button.top() < px(36.));
        assert!(settings_button.left() > window.viewport_size().width * 0.85);
        window.click("open-settings", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        window.click("settings-vim", cx);
        window.render_frame(cx);
        assert!(workspace.read(cx).settings.vim_enabled);
        window.within("dialog").click("close", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, _| {
        assert!(window.find("vim-mode").bounds().left() > window.viewport_size().width / 2.);
        assert!(window.find("vim-mode").bounds().top() > window.viewport_size().height - px(30.));
        assert_eq!(
            window.find("status-bar").bounds().bottom(),
            window.viewport_size().height
        );
    })
    .unwrap();
    settle_surface(&mut app, handle);
    // Native thin-caret blink cycles cannot alter the solid modal block.
    let (glyph, viewport) = app
        .update_window(handle, |_, window, cx| {
            (
                workspace.read(cx).documents[0]
                    .editor
                    .read(cx)
                    .range_to_bounds(&(0..1))
                    .unwrap(),
                window.viewport_size(),
            )
        })
        .unwrap();
    let mut frames = Vec::new();
    for delay in [350, 500, 500] {
        app.advance_clock(Duration::from_millis(delay));
        app.run_until_parked();
        app.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        let image = app.capture_screenshot(handle).unwrap();
        let scale = image.width() as f32 / f32::from(viewport.width);
        let x = (f32::from(glyph.left()) * scale).floor() as u32;
        let y = (f32::from(glyph.top()) * scale).floor() as u32;
        frames.push(
            image::imageops::crop_imm(
                &image,
                x.saturating_sub(1),
                y.saturating_sub(1),
                (f32::from(glyph.size.width) * scale).ceil() as u32 + 3,
                (f32::from(glyph.size.height) * scale).ceil() as u32 + 3,
            )
            .to_image(),
        );
    }
    assert_eq!(
        frames[0], frames[1],
        "Normal block changed on a blink cycle"
    );
    assert_eq!(frames[1], frames[2]);
    assert!(
        frames[0]
            .pixels()
            .filter(|p| p[0] < 100 && (100..170).contains(&p[1]) && p[2] > 180)
            .count()
            > 40,
        "Expected a filled block, not a thin caret"
    );
    app.update_window(handle, |_, window, cx| {
        window.press("ctrl-g", cx);
        window.render_frame(cx);
        window.input("2:4", cx);
        window.press("enter", cx);
        assert!(!workspace.read(cx).goto_open);
        assert_eq!(
            workspace.read(cx).documents[0]
                .editor
                .read(cx)
                .cursor_position(),
            lsp_types::Position::new(1, 3)
        );
        window.press("alt-z", cx);
        assert!(!workspace.read(cx).settings.soft_wrap);
        workspace.update(cx, |view, cx| view.open_document(3, window, cx));
        window.press("ctrl-shift-tab", cx);
        assert_eq!(workspace.read(cx).active, 0);
        window.press("ctrl-tab", cx);
        assert_eq!(workspace.read(cx).active, 3);
        window.press("ctrl-shift-tab", cx);
        choose_zoom(window, cx, 3);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    let width_zoom = app.update(|cx| workspace.read(cx).zoom);
    app.update_window(handle, |_, window, cx| {
        let view = workspace.read(cx);
        let layout = view.preview_layout(view.zoom);
        let width = layout.pages.iter().map(|p| p.size.width).max().unwrap();
        assert!((f32::from(width + px(48.) - view.preview_viewport.width)).abs() < 0.1);
        window.resize(size(px(1720.), px(1080.)));
        window.bounds_changed(cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    let before_drag_zoom = app.update(|cx| workspace.read(cx).zoom);
    app.update_window(handle, |_, window, cx| {
        assert_ne!(workspace.read(cx).zoom, width_zoom);
        let left = workspace.read(cx).preview_scroll.bounds().left();
        window.drag(point(left, px(300.)), point(left - px(100.), px(300.)), cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert_ne!(
            workspace.read(cx).zoom,
            before_drag_zoom,
            "Dragging the divider must refit the page"
        );
        choose_zoom(window, cx, 4);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        let view = workspace.read(cx);
        let layout = view.preview_layout(view.zoom);
        let height = layout.pages.iter().map(|p| p.size.height).max().unwrap();
        assert!((f32::from(height + px(48.) - view.preview_viewport.height)).abs() < 0.1);
        window.resize(size(px(1600.), px(920.)));
        window.bounds_changed(cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, _, cx| {
        let view = workspace.read(cx);
        let layout = view.preview_layout(view.zoom);
        let height = layout.pages.iter().map(|p| p.size.height).max().unwrap();
        assert!((f32::from(height + px(48.) - view.preview_viewport.height)).abs() < 0.1);
        assert_eq!(view.preview_fit, FitMode::Page);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        let view = workspace.read(cx);
        assert_eq!(view.preview_fit, FitMode::Page);
        for page in view.preview_layout(view.zoom).pages {
            assert!(page.size.width + px(48.) <= view.preview_viewport.width + px(0.1));
            assert!(page.size.height + px(48.) <= view.preview_viewport.height + px(0.1));
        }
        choose_zoom(window, cx, 10); // 200%, clearing the responsive fit mode.
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert_eq!(workspace.read(cx).preview_fit, FitMode::Manual);
        assert_eq!(workspace.read(cx).zoom, 2.);
        let editor = workspace.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| state.focus(window, cx));
        let text = editor.read(cx).value();
        window.press("ctrl--", cx);
        assert_eq!(workspace.read(cx).zoom, 1.6);
        window.press("ctrl-+", cx);
        assert_eq!(workspace.read(cx).zoom, 2.);
        window.press("ctrl-0", cx);
        assert_eq!(workspace.read(cx).zoom, 1.);
        assert_eq!(editor.read(cx).value(), text);
        window.click("zoom-menu", cx);
        window.render_frame(cx);
        assert!(window.find("popup-menu").visible());
    })
    .unwrap();
    settle_surface(&mut app, handle);
    if let Ok(path) = std::env::var("FOLIO_ZOOM_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(window.try_find("popup-menu").is_none());
        let view = workspace.read(cx);
        let text = view.documents[view.active].editor.read(cx).value();
        window.press("ctrl-alt-p", cx);
        assert!(!workspace.read(cx).preview_visible);
        window.press("ctrl-alt-p", cx);
        assert!(workspace.read(cx).preview_visible);
        assert_eq!(
            workspace.read(cx).documents[0].editor.read(cx).value(),
            text,
            "Preview shortcuts must not become Vim paste commands"
        );
        window.click("toggle-preview", cx);
        window.render_frame(cx);
        assert!(!workspace.read(cx).preview_visible);
        assert!(window.try_find("preview-scroll").is_none());
        window.click("toggle-preview", cx);
        assert!(workspace.read(cx).preview_visible);
        window.render_frame(cx);
        window.click("pop-out-preview", cx);
    })
    .unwrap();
    app.run_until_parked();
    let floating = app.update(|cx| {
        assert_eq!(cx.windows().len(), 2);
        let view = workspace.read(cx);
        assert!(!view.preview_visible);
        view.detached_preview.unwrap()
    });
    settle_surface(&mut app, handle);
    settle_surface(&mut app, floating);
    app.update_window(handle, |_, window, _| {
        assert!(window.try_find("preview-scroll").is_none())
    })
    .unwrap();
    app.update_window(floating, |_, window, cx| {
        window.press("ctrl-+", cx);
        assert_eq!(workspace.read(cx).zoom, 1.25);
        window.press("ctrl-0", cx);
        assert_eq!(workspace.read(cx).zoom, 1.);
        choose_zoom(window, cx, 3);
        window.resize(size(px(1040.), px(760.)));
        window.bounds_changed(cx);
    })
    .unwrap();
    settle_surface(&mut app, floating);
    let (old_page, cache) = app.update(|cx| {
        let view = workspace.read(cx);
        let width = view.preview_layout(view.zoom).pages[0].size.width;
        assert!((f32::from(width + px(48.) - view.preview_viewport.width)).abs() < 0.1);
        (
            view.preview.as_ref().unwrap().pages[0].id,
            view.tile_cache.clone(),
        )
    });
    app.update_window(handle, |_, window, cx| {
        workspace.read(cx).documents[0]
            .editor
            .clone()
            .update(cx, |state, cx| {
                state.replace_all("Live updates in a detached preview.", window, cx)
            });
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        !view.compiling
            && view.compile_error.is_none()
            && view
                .preview
                .as_ref()
                .is_some_and(|preview| preview.pages[0].id != old_page)
    });
    settle_surface(&mut app, floating);
    app.update_window(floating, |_, window, cx| {
        assert!(window.find(("preview-page", 0usize)).visible());
        assert!(Rc::ptr_eq(&cache, &workspace.read(cx).tile_cache));
        window.remove_window();
    })
    .unwrap();
    app.run_until_parked();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(workspace.read(cx).detached_preview.is_none());
        assert!(workspace.read(cx).preview_visible);
        assert!(window.find("preview-scroll").visible());
        window.click("pop-out-preview", cx);
    })
    .unwrap();
    let floating = app.update(|cx| workspace.read(cx).detached_preview.unwrap());
    settle_surface(&mut app, floating);
    app.update_window(floating, |_, window, cx| {
        window.click("pop-out-preview", cx)
    })
    .unwrap();
    app.run_until_parked();
    settle_surface(&mut app, handle);
    assert!(
        app.update(|cx| workspace.read(cx).preview_visible
            && workspace.read(cx).detached_preview.is_none())
    );
    if let Ok(path) = std::env::var("TYPST_EDITOR_WORKBENCH_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| window.click("pop-out-preview", cx))
        .unwrap();
    let floating = app.update(|cx| workspace.read(cx).detached_preview.unwrap());
    settle_surface(&mut app, floating);
    app.update_window(handle, |_, window, cx| {
        window.click("toggle-preview", cx); // Dock, then hide from the status bar.
        window.render_frame(cx);
        window.click("toggle-preview", cx);
    })
    .unwrap();
    app.run_until_parked();
    assert!(
        app.update(|cx| !workspace.read(cx).preview_visible
            && workspace.read(cx).detached_preview.is_none())
    );
    // Closing the editor also closes its owned floating preview.
    app.update_window(handle, |_, window, cx| {
        workspace.update(cx, |view, cx| {
            view.pop_out_preview(&PopOutPreview, window, cx)
        });
    })
    .unwrap();
    app.update_window(handle, |_, window, _| window.remove_window())
        .unwrap();
    app.run_until_parked();
    assert_eq!(app.update(|cx| cx.windows().len()), 0);
}

#[::core::prelude::v1::test]
fn compilation_dialog_reports_unopened_invoice_imports_and_backend_failures() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Invoice Library With Spaces");
    for (entry, contents) in [
        (
            "templates/Rechnung.typ",
            include_str!("../../templates/invoice-pro/templates/Rechnung.typ"),
        ),
        (
            "lib/rechnung.typ",
            include_str!("../../templates/invoice-pro/lib/rechnung.typ"),
        ),
        (
            "lib/kontakte.typ",
            include_str!("../../templates/invoice-pro/lib/kontakte.typ"),
        ),
    ] {
        let path = root.join(entry);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let template = crate::invoice::templates(&root).unwrap().remove(0);
    let entry = crate::invoice::create(&root, &template, "1", "First invoice").unwrap();
    let contacts = root.join("lib/kontakte.typ");
    let valid_contacts = fs::read_to_string(&contacts).unwrap();
    fs::write(
        &contacts,
        format!("{valid_contacts}\n#let broken = missing_contact_for_test\n"),
    )
    .unwrap();
    let contacts_uri = lsp::file_uri(&contacts);
    let project = crate::project::Project::document(&entry).unwrap();
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
    });
    let (handle, workspace) = app
        .update(|cx| {
            open_window(
                WindowOptions {
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Workspace::open(project, window, cx)),
            )
        })
        .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        view.compile_error.is_some()
            && view.lsp.all_diagnostics().iter().any(|(uri, diagnostic)| {
                uri == &contacts_uri && diagnostic.message.contains("missing_contact_for_test")
            })
    });
    app.update(|cx| {
        assert_eq!(
            workspace.read(cx).documents.len(),
            1,
            "An imported module should not need an open source buffer to report errors"
        )
    });
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| window.click("preview-warning", cx))
        .unwrap();
    settle_surface(&mut app, handle);
    if let Ok(path) = std::env::var("FOLIO_COMPILE_ERRORS_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        assert!(window.find("compile-errors").visible());
        window.click("copy-compilation-errors", cx);
        let details = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap();
        assert!(details.contains("lib/kontakte.typ:"), "{details}");
        assert!(details.contains("missing_contact_for_test"), "{details}");
        window.click("compilation-error-source", cx);
    })
    .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        assert!(window.try_find("compile-errors").is_none());
        let editor = workspace.update(cx, |view, _| {
            assert_eq!(view.documents[view.active].path, "lib/kontakte.typ");
            view.documents[view.active].editor.clone()
        });
        editor.update(cx, |editor, cx| {
            editor.replace_all(valid_contacts, window, cx)
        });
    })
    .unwrap();
    wait_language(&mut app, &workspace, |view, _| {
        view.preview.is_some()
            && view.compile_error.is_none()
            && view.lsp.diagnostics(&contacts_uri).is_empty()
    });
    app.update(|cx| {
        workspace.update(cx, |view, cx| {
            view.compile_error =
                Some("Could not start Tinymist\nPermission denied: bundled tool".into());
            cx.notify();
        })
    });
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| window.click("preview-warning", cx))
        .unwrap();
    settle_surface(&mut app, handle);
    app.update_window(handle, |_, window, cx| {
        window.click("copy-compilation-errors", cx);
        let details = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap();
        assert!(
            details.contains("Permission denied: bundled tool"),
            "{details}"
        );
        window.close_dialog(cx);
        window.remove_window();
    })
    .unwrap();
    app.run_until_parked();
}
