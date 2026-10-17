use super::*;
use gpui_kit::test::TestWindowExt;
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};
fn content(window: &Window, cx: &App) -> Entity<Workspace> {
    gpui_kit::base::Root::read(window, cx)
        .view()
        .clone()
        .downcast::<Home>()
        .ok()
        .unwrap()
        .read(cx)
        .workspace
        .clone()
        .unwrap()
}
#[track_caller]
fn wait_for(
    app: &mut HeadlessAppContext,
    editor: AnyWindowHandle,
    condition: impl Fn(&serde_json::Value) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        app.run_until_parked();
        let ready = app
            .update_window(editor, |_, window, cx| {
                window.render_frame(cx);
                let state = content(window, cx).read(cx).snapshot(cx);
                assert_eq!(state["lsp_failed"], false, "{state}");
                condition(&state)
            })
            .unwrap();
        if ready {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for editor state"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[::core::prelude::v1::test]
fn opening_and_refreshing_libraries_never_populates_them() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("Missing");
    let empty = directory.path().join("Empty");
    let populated = directory.path().join("Existing");
    fs::create_dir(&empty).unwrap();
    fs::create_dir_all(populated.join("templates")).unwrap();
    fs::write(populated.join("templates/Custom.typ"), "Custom template").unwrap();
    fs::write(populated.join("Custom_1_Example.typ"), "My document").unwrap();
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::configure_theme(cx);
    });
    let (handle, home) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    Home::with_library(
                        Library {
                            root: missing.clone(),
                        },
                        None,
                        None,
                        window,
                        cx,
                    )
                })
            },
        )
        .unwrap()
    });
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        home.update(cx, |this, cx| {
            this.refresh(cx);
            assert!(!missing.exists());
            this.set_library(empty.clone(), cx);
            this.refresh(cx);
            assert!(this.error.is_none(), "{:?}", this.error);
            assert!(this.templates.is_empty());
            assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
            this.set_library(populated.clone(), cx);
            this.refresh(cx);
            assert!(this.error.is_none(), "{:?}", this.error);
            assert_eq!(this.records.len(), 1);
            assert_eq!(this.templates.len(), 1);
            assert_eq!(fs::read_dir(&populated).unwrap().count(), 2);
            assert_eq!(
                fs::read_dir(populated.join("templates")).unwrap().count(),
                1
            );
            assert_eq!(
                fs::read_to_string(populated.join("templates/Custom.typ")).unwrap(),
                "Custom template"
            );
            assert_eq!(
                fs::read_to_string(populated.join("Custom_1_Example.typ")).unwrap(),
                "My document"
            );
            this.set_library(missing.clone(), cx);
            assert!(this.error.is_some());
            assert_eq!(this.library.root, populated);
            assert!(!missing.exists());
        });
    })
    .unwrap();
}
#[::core::prelude::v1::test]
fn structured_library_creation_filters_exports_history_and_saved_editor_buffers() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("preferences.json");
    let root = directory.path().join("Documents");
    invoice::create_test_library(&root).unwrap();
    for (entry, source) in [
        (
            "templates/Rechnung.typ",
            include_str!("../../templates/invoice-pro/templates/Rechnung.typ"),
        ),
        (
            "templates/Angebot.typ",
            include_str!("../../templates/invoice-pro/templates/Angebot.typ"),
        ),
        (
            "lib/kontakte.typ",
            include_str!("../../templates/invoice-pro/lib/kontakte.typ"),
        ),
        (
            "lib/rechnung.typ",
            include_str!("../../templates/invoice-pro/lib/rechnung.typ"),
        ),
    ] {
        let path = root.join(entry);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    let library = Library { root: root.clone() };
    let mut app = HeadlessAppContext::with_platform(
        platform::current_platform(true).text_system(),
        Arc::new(crate::branding::Assets),
        platform::current_headless_renderer,
    );
    app.update(|cx| {
        init(cx);
        crate::prompts::init(cx);
        crate::configure_theme(cx);
        crate::bind_editor_keys(cx);
        cx.set_global(Windows::default());
    });
    let (handle, home) = app.update(|cx| {
        open_window(
            WindowOptions {
                show: false,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1320.), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| Home::with_library(library, Some(config.clone()), None, window, cx))
            },
        )
        .unwrap()
    });
    app.update(|cx| cx.global_mut::<Windows>().home = Some((handle, home.downgrade())));
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("invoices-nav").is_none());
        assert!(window.try_find("recents-nav").is_none());
        window.click("new-invoice", cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        home.update(cx, |this, cx| {
            assert_eq!(this.selected.as_ref().unwrap().name, "Rechnung");
            assert_eq!(this.number.read(cx).value(), "1");
        });
        window.click("document-number", cx);
        window.press("ctrl-a", cx);
        window.input("42", cx);
        window.click("document-title", cx);
        window.input("Design", cx);
        window.render_frame(cx);
    })
    .unwrap();
    if let Ok(path) = std::env::var("FOLIO_CREATE_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| window.click("create-document", cx))
        .unwrap();
    app.run_until_parked();
    let source = root.join("documents/Rechnung/Rechnung_42_Design.typ");
    assert!(source.is_file());
    assert!(!root.join("Rechnung_42_Design").exists());
    app.update(|cx| {
        let view = home.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.records.len(), 1);
        assert!(!view.create_open);
        assert_eq!(cx.windows().len(), 1);
    });
    let editor = app.update(|cx| cx.global::<Windows>().projects[&source]);
    wait_for(&mut app, editor, |state| {
        state["preview_pages"].as_u64().unwrap_or(0) > 0 && state["lsp_ready"] == true
    });
    app.update_window(editor, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("source-tabs").is_none());
        assert!(window.try_find("toggle-sidebar").is_none());
        assert!(window.try_find(("close-tab", 0usize)).is_none());
        window.dispatch_action(Box::new(crate::ToggleSidebar), cx);
        let workspace = content(window, cx);
        let state = workspace.read(cx).snapshot(cx);
        assert_eq!(state["library_editor"], true);
        assert_eq!(state["sidebar_visible"], false);
        assert_eq!(state["tabs_visible"], false);
        assert_eq!(state["compile_failed"], false, "{state}");
        assert_eq!(
            state["preview_scroll"][1], 0.0,
            "The first preview starts at the top"
        );
    })
    .unwrap();
    if let Ok(path) = std::env::var("FOLIO_INVOICE_SCREENSHOT") {
        app.capture_screenshot(editor).unwrap().save(path).unwrap();
    }
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(Box::new(crate::ExportPdf), cx)
    })
    .unwrap();
    wait_for(&mut app, editor, |state| {
        !state["export_busy"].as_bool().unwrap()
            && state["export_message"]
                .as_str()
                .is_some_and(|s| s.contains("PDF saved"))
    });
    assert!(root.join("pdf/Rechnung/Rechnung_42_Design.pdf").is_file());
    let first = fs::read(root.join("pdf/Rechnung/Rechnung_42_Design.pdf")).unwrap();
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::workspace::commands::RunCommand::new(
                crate::workspace::commands::Command::FocusEditor {},
            )),
            cx,
        );
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(editor, |_, window, cx| {
        window.input("// Unsaved change before export\n", cx);
        window.dispatch_action(Box::new(crate::ExportPdf), cx);
    })
    .unwrap();
    wait_for(&mut app, editor, |state| {
        !state["export_busy"].as_bool().unwrap()
            && state["export_message"]
                .as_str()
                .is_some_and(|s| s.contains("__v0002.pdf"))
    });
    assert_eq!(
        fs::read(root.join("pdf/Rechnung/Rechnung_42_Design.pdf")).unwrap(),
        first
    );
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .contains("Unsaved change before export")
    );
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(Box::new(crate::PdfHistory), cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(editor, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("open-pdf-version", 1usize)).is_some());
        assert!(window.try_find(("open-pdf-version", 2usize)).is_some());
    })
    .unwrap();
    if let Ok(path) = std::env::var("FOLIO_HISTORY_SCREENSHOT") {
        app.capture_screenshot(editor).unwrap().save(path).unwrap();
    }
    app.update_window(editor, |_, window, cx| window.close_dialog(cx))
        .unwrap();
    let quote = invoice::templates(&root)
        .unwrap()
        .into_iter()
        .find(|t| t.name == "Angebot")
        .unwrap();
    invoice::create(&root, &quote, "1", "Website").unwrap();
    app.update_window(editor, |_, window, cx| window.click("open-library", cx))
        .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        assert!(home.read(cx).workspace.is_none());
        assert_eq!(cx.windows().len(), 1);
    });
    app.update_window(handle, |_, window, cx| {
        home.update(cx, |this, cx| this.refresh(cx));
        window.render_frame(cx);
    })
    .unwrap();
    if let Ok(path) = std::env::var("FOLIO_LIBRARY_SCREENSHOT") {
        app.capture_screenshot(handle).unwrap().save(path).unwrap();
    }
    app.update_window(handle, |_, window, cx| {
        window.within("document-types").click(1usize, cx);
        window.render_frame(cx);
        home.update(cx, |this, _| {
            assert_eq!(this.filter.as_deref(), Some("Angebot"))
        });
        window.click("new-invoice", cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        home.update(cx, |this, cx| {
            assert_eq!(this.selected.as_ref().unwrap().name, "Angebot");
            assert_eq!(this.number.read(cx).value(), "2");
        });
        window.click("choose-template", cx);
        window.render_frame(cx);
        window.within("popup-menu").click(1usize, cx);
        window.render_frame(cx);
        home.update(cx, |this, cx| {
            assert_eq!(this.selected.as_ref().unwrap().name, "Rechnung");
            assert_eq!(this.number.read(cx).value(), "43");
        });
        window.click("cancel-create", cx);
    })
    .unwrap();
    // Reopen in the same window, then verify debounced autosave and immediate
    // save on back, including closing a detached preview.
    app.update_window(handle, |_, window, cx| {
        home.update(cx, |this, cx| this.open_document(&source, window, cx));
    })
    .unwrap();
    wait_for(&mut app, editor, |state| {
        state["preview_pages"].as_u64().unwrap_or(0) > 0
    });
    app.update_window(editor, |_, window, cx| {
        assert_eq!(
            content(window, cx).read(cx).snapshot(cx)["documents"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        window.input("// Autosaved after typing\n", cx);
    })
    .unwrap();
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(800));
    wait_for(&mut app, editor, |state| {
        state["document"]["dirty"] == false
    });
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .contains("Autosaved after typing")
    );
    // A background save must not overwrite external edits. Going back also
    // stays in the editor when saving fails, preserving the dirty buffer.
    let saved = fs::read_to_string(&source).unwrap();
    fs::write(&source, "// Changed outside Folio\n").unwrap();
    app.update_window(editor, |_, window, cx| {
        window.input("// Keep these edits\n", cx)
    })
    .unwrap();
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(800));
    wait_for(&mut app, editor, |state| {
        state["errors"]["save"]
            .as_str()
            .is_some_and(|error| error.contains("changed on disk"))
    });
    app.update_window(editor, |_, window, cx| window.click("open-library", cx))
        .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        assert!(home.read(cx).workspace.is_some());
        assert_eq!(cx.windows().len(), 1);
    });
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "// Changed outside Folio\n"
    );
    fs::write(&source, saved).unwrap();
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(Box::new(crate::Save), cx)
    })
    .unwrap();
    wait_for(&mut app, editor, |state| {
        state["document"]["dirty"] == false && state["errors"]["save"].is_null()
    });
    // Autosave can be disabled in Settings; going back still flushes edits.
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::workspace::commands::RunCommand::new(
                crate::workspace::commands::Command::Configure {
                    settings: crate::workspace::commands::SettingsPatch {
                        autosave: Some(false),
                        ..Default::default()
                    },
                },
            )),
            cx,
        );
    })
    .unwrap();
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(Box::new(crate::PopOutPreview), cx);
    })
    .unwrap();
    wait_for(&mut app, editor, |state| state["preview_detached"] == true);
    app.update_window(editor, |_, window, cx| {
        window.input("// Saved immediately on back\n", cx);
    })
    .unwrap();
    app.run_until_parked();
    app.advance_clock(Duration::from_millis(900));
    app.run_until_parked();
    assert!(
        !fs::read_to_string(&source)
            .unwrap()
            .contains("Saved immediately on back")
    );
    app.update_window(editor, |_, window, cx| {
        window.dispatch_action(Box::new(crate::ShowLibrary), cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        assert!(home.read(cx).workspace.is_none());
        assert_eq!(cx.windows().len(), 1);
    });
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .contains("Saved immediately on back")
    );
    app.update(|cx| home.update(cx, |this, cx| this.set_library(root.clone(), cx)));
    assert_eq!(Library::load(&config).unwrap().root, root);
    let contacts = root.join("lib/kontakte.typ");
    fs::write(
        &contacts,
        "#let kontakte = (freundlich: (name: \"Firma Freundlich\"),)\n",
    )
    .unwrap();
    app.update_window(handle, |_, window, cx| {
        window.dispatch_action(Box::new(crate::OpenContacts), cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        let state = content(window, cx).read(cx).snapshot(cx);
        assert_eq!(state["document"]["path"], "lib/kontakte.typ");
        assert_eq!(state["preview_visible"], false);
        assert!(window.try_find("source-tabs").is_none());
        assert!(window.try_find("toggle-sidebar").is_none());
        window.input("// Updated contact details\n", cx);
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        // In the single-document editor Ctrl+W saves and returns to the library.
        window.dispatch_action(Box::new(crate::CloseTab), cx)
    })
    .unwrap();
    app.run_until_parked();
    assert!(
        fs::read_to_string(&contacts)
            .unwrap()
            .contains("Updated contact details")
    );
    assert_eq!(invoice::scan(&root).unwrap().len(), 2);
    app.update(|cx| {
        assert!(home.read(cx).workspace.is_none());
        assert_eq!(cx.windows().len(), 1);
    });
    app.update_window(handle, |_, window, cx| {
        home.update(cx, |this, cx| this.open_document(&source, window, cx));
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        window.input("// Autosaved when closing\n", cx)
    })
    .unwrap();
    app.run_until_parked();
    app.update_window(handle, |_, window, cx| {
        window.dispatch_action(Box::new(crate::CloseProject), cx)
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| assert!(cx.windows().is_empty()));
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .contains("Autosaved when closing")
    );
}
