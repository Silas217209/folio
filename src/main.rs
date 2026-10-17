mod branding;
mod home;
mod invoice;
mod library;
mod lsp;
mod lsp_provider;
mod page_view;
mod pdf_history;
mod preview;
mod preview_layout;
mod project;
mod prompts;
mod runtime;
mod settings;
mod share;
mod syntax;
#[cfg(test)]
mod test_support;
mod vim;
mod workspace;

use gpui_kit::component::{
    Theme, ThemeMode,
    input::{GoToDefinition, Redo, ShowCompletions},
};
use gpui_kit::*;

actions!(
    folio,
    [
        Save,
        Quit,
        ShowLibrary,
        NewInvoice,
        OpenFolder,
        OpenContacts,
        ExportPdf,
        EmailPdf,
        SharePdf,
        PdfHistory,
        ToggleSidebar,
        Recompile,
        CloseTab,
        CloseProject,
        FormatDocument,
        RenameSymbol,
        FindReferences,
        SignatureHelp,
        ToggleVim,
        OpenSettings,
        TogglePreview,
        PopOutPreview,
        GoToLine,
        ToggleWrap,
        NextTab,
        PreviousTab,
        PreviewZoomOut,
        PreviewZoomIn,
        PreviewZoomReset,
        PreviewFitWidth,
        PreviewFitPage
    ]
);

fn primary_key(key: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "macos")
        && !matches!(
            key,
            "ctrl-space" | "ctrl-shift-space" | "ctrl-tab" | "ctrl-shift-tab"
        )
    {
        key.replacen("ctrl-", "cmd-", 1).into()
    } else {
        key.into()
    }
}

fn macos_menus(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| home::quit(cx));
    cx.set_menus([
        Menu::new("Folio").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Folio", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Invoice…", NewInvoice),
            MenuItem::action("Open Library…", OpenFolder),
            MenuItem::action("Library", ShowLibrary),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Export PDF…", ExportPdf),
            MenuItem::action("Close Document", CloseTab),
        ]),
    ]);
}

fn bind_editor_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(primary_key("ctrl-s").as_ref(), Save, None),
        KeyBinding::new(primary_key("ctrl-shift-o").as_ref(), OpenFolder, None),
        KeyBinding::new(primary_key("ctrl-n").as_ref(), NewInvoice, None),
        KeyBinding::new(primary_key("ctrl-shift-e").as_ref(), ExportPdf, None),
        KeyBinding::new(primary_key("ctrl-shift-h").as_ref(), ShowLibrary, None),
        KeyBinding::new(primary_key("ctrl-b").as_ref(), ToggleSidebar, None),
        KeyBinding::new(primary_key("ctrl-enter").as_ref(), Recompile, None),
        KeyBinding::new(primary_key("ctrl-w").as_ref(), CloseTab, None),
        KeyBinding::new(primary_key("ctrl-shift-w").as_ref(), CloseProject, None),
        KeyBinding::new(
            primary_key("ctrl-space").as_ref(),
            ShowCompletions,
            Some("Input"),
        ),
        KeyBinding::new("f12", GoToDefinition, Some("Input")),
        KeyBinding::new("f2", RenameSymbol, None),
        KeyBinding::new("shift-f12", FindReferences, None),
        KeyBinding::new(
            primary_key("ctrl-shift-space").as_ref(),
            SignatureHelp,
            None,
        ),
        KeyBinding::new(primary_key("ctrl-shift-i").as_ref(), FormatDocument, None),
        KeyBinding::new(primary_key("ctrl-shift-z").as_ref(), Redo, Some("Input")),
        KeyBinding::new("f6", ToggleVim, None),
        KeyBinding::new(primary_key("ctrl-,").as_ref(), OpenSettings, None),
        KeyBinding::new(primary_key("ctrl-alt-p").as_ref(), TogglePreview, None),
        KeyBinding::new(
            primary_key("ctrl-alt-shift-p").as_ref(),
            PopOutPreview,
            None,
        ),
        KeyBinding::new(primary_key("ctrl-g").as_ref(), GoToLine, None),
        KeyBinding::new("alt-z", ToggleWrap, None),
        KeyBinding::new(primary_key("ctrl-tab").as_ref(), NextTab, None),
        KeyBinding::new(primary_key("ctrl-shift-tab").as_ref(), PreviousTab, None),
        KeyBinding::new(primary_key("ctrl--").as_ref(), PreviewZoomOut, None),
        KeyBinding::new(primary_key("ctrl-=").as_ref(), PreviewZoomIn, None),
        KeyBinding::new(primary_key("ctrl-shift-=").as_ref(), PreviewZoomIn, None),
        KeyBinding::new(primary_key("ctrl-+").as_ref(), PreviewZoomIn, None),
        KeyBinding::new(primary_key("ctrl-0").as_ref(), PreviewZoomReset, None),
    ]);
}

fn configure_theme(cx: &mut App) {
    Theme::change(ThemeMode::Light, None, cx);
    Theme::update(cx, |theme| {
        theme.font_size = px(14.);
        theme.mono_font_size = px(13.);
        theme.colors.primary = rgb(0x3478d4).into();
        theme.colors.tab_bar = theme.colors.sidebar;
        theme.colors.tab_active = theme.colors.background;
        theme.colors.tab_active_foreground = theme.colors.foreground;
    });
}

fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--render-icon")
    {
        let result = std::env::args_os()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("Usage: folio --render-icon OUTPUT.png"))
            .and_then(|path| branding::write_icon(std::path::Path::new(&path)));
        if let Err(error) = result {
            eprintln!("Could not render Folio icon: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--check-runtime")
    {
        match runtime::check() {
            Ok(report) => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
            Err(error) => {
                eprintln!("Folio runtime check failed: {error:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    let application = application().with_assets(branding::Assets);
    let (open_urls, urls) = async_channel::unbounded::<Vec<String>>();
    if cfg!(target_os = "macos") {
        application.on_open_urls(move |values| {
            let _ = open_urls.try_send(values);
        });
        application.on_reopen(home::show);
    }
    application.run(move |cx| {
        cx.set_app_identity(branding::APP_ID, branding::NAME);
        init(cx);
        prompts::init(cx);
        configure_theme(cx);
        bind_editor_keys(cx);
        if cfg!(target_os = "macos") {
            macos_menus(cx);
            cx.spawn(async move |cx| {
                while let Ok(urls) = urls.recv().await {
                    for value in urls {
                        if let Ok(url) = url::Url::parse(&value)
                            && let Ok(path) = url.to_file_path()
                        {
                            cx.update(|cx| home::open_path(path, cx));
                        }
                    }
                }
            })
            .detach();
        }
        home::show(cx);
        if let Some(path) = std::env::args_os().nth(1) {
            home::open_path(std::path::PathBuf::from(path), cx);
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
