use super::*;

// There is exactly one visible preview surface. Moving it between windows keeps
// the same vector pages, scroll position, fit mode, textures, and compiler.
pub(super) struct DetachedPreview {
    workspace: Entity<Workspace>,
    focus: FocusHandle,
    _subscription: Subscription,
}
impl DetachedPreview {
    fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let subscription = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self {
            workspace,
            focus,
            _subscription: subscription,
        }
    }
}
impl Render for DetachedPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus.clone();
        self.workspace.update(cx, |workspace, cx| {
            v_flex()
                .track_focus(&focus)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    focus.focus(window, cx)
                })
                .size_full()
                .on_action(cx.listener(Workspace::run_command))
                .on_action(cx.listener(Workspace::show_compilation_errors))
                .on_action(cx.listener(Workspace::zoom_out))
                .on_action(cx.listener(Workspace::zoom_in))
                .on_action(cx.listener(Workspace::zoom_reset))
                .on_action(cx.listener(Workspace::fit_width))
                .on_action(cx.listener(Workspace::fit_page))
                .on_action(cx.listener(Workspace::toggle_preview))
                .on_action(cx.listener(Workspace::pop_out_preview))
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(
                    TitleBar::new()
                        .h(px(36.))
                        .child(div().text_size(px(12.)).child("Folio · Preview")),
                )
                .child(div().flex_1().min_h_0().child(workspace.preview_pane(cx)))
        })
    }
}
impl Workspace {
    pub(super) fn preview_viewport_size(&self) -> Size<Pixels> {
        if self.preview_viewport.width > px(0.) {
            self.preview_viewport
        } else {
            self.preview_scroll.bounds().size
        }
    }
    fn page_dimensions(&self) -> Vec<(f32, f32)> {
        self.preview
            .as_ref()
            .map(|p| p.pages.iter().map(|p| (p.width(), p.height())).collect())
            .unwrap_or_default()
    }
    pub(super) fn set_fit(&mut self, mode: FitMode, cx: &mut Context<Self>) {
        self.preview_fit = mode;
        self.refresh_fit(cx);
    }
    pub(super) fn refresh_fit(&mut self, cx: &mut Context<Self>) {
        let zoom = self.preview_fit.zoom(
            &self.page_dimensions(),
            self.preview_viewport_size(),
            self.zoom,
        );
        if self.zoom != zoom {
            let follow = self.follow_edit;
            self.apply_preview_zoom(zoom, None, cx);
            self.follow_edit = follow;
        }
        cx.notify();
    }
    fn resize_preview(&mut self, viewport: Size<Pixels>, cx: &mut Context<Self>) {
        if viewport == self.preview_viewport
            || viewport.width <= px(0.)
            || viewport.height <= px(0.)
        {
            return;
        }
        let old = self.preview_layout(self.zoom);
        self.preview_viewport = viewport;
        if self.preview_fit != FitMode::Manual {
            self.scroll_animation = None;
            self.zoom = self
                .preview_fit
                .zoom(&self.page_dimensions(), viewport, self.zoom);
            let next = self.preview_layout(self.zoom);
            self.preview_scroll.set_offset(old.zoom_offset(
                &next,
                self.preview_scroll.offset(),
                point(viewport.width / 2., px(24.)),
                viewport,
            ));
        }
        cx.notify();
    }
    pub(super) fn measure_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = cx.weak_entity();
        canvas(
            move |bounds, window, cx| {
                let size = bounds.size;
                if workspace
                    .read_with(cx, |view, _| view.preview_viewport != size)
                    .unwrap_or(false)
                {
                    let workspace = workspace.clone();
                    window.defer(cx, move |_, cx| {
                        let _ = workspace.update(cx, |view, cx| view.resize_preview(size, cx));
                    });
                }
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0()
        .size_full()
    }
    pub(super) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.detached_preview.is_some() {
            self.dock_preview(window, cx);
        } else {
            self.preview_visible = !self.preview_visible;
            cx.notify();
        }
    }
    fn close_detached(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.detached_preview.take() {
            if handle == window.window_handle() {
                window.remove_window();
            } else {
                // Window-close observers update this workspace too. Release its
                // current borrow before closing a preview from the main window.
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                });
            }
        }
    }
    pub(super) fn dock_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_opening = false;
        self.preview_visible = true;
        self.close_detached(window, cx);
        let main = self.main_window;
        cx.defer(move |cx| {
            let _ = main.update(cx, |_, window, _| window.activate_window());
        });
        cx.notify();
    }
    pub(super) fn pop_out_preview(
        &mut self,
        _: &PopOutPreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self.detached_preview {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            });
            return;
        }
        if self.preview_opening {
            return;
        }
        self.preview_opening = true;
        let workspace = cx.entity();
        // open_window draws its first frame immediately. Release the workspace
        // borrow before the new view reads it to construct that frame.
        cx.defer(move |cx| {
            if !workspace.read(cx).preview_opening {
                return;
            }
            let main = workspace.read(cx).main_window;
            if !cx.windows().contains(&main) {
                workspace.update(cx, |this, _| this.preview_opening = false);
                return;
            }
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(800.), px(900.)),
                    cx,
                ))),
                window_min_size: Some(size(px(400.), px(320.))),
                app_id: Some(crate::branding::APP_ID.into()),
                icon: Some(crate::branding::window_icon()),
                ..TitleBar::window_options()
            };
            let owner = workspace.clone();
            let result = open_window(options, cx, |window, cx| {
                window.set_window_title("Folio — Preview");
                cx.new(|cx| DetachedPreview::new(owner, window, cx))
            });
            workspace.update(cx, |this, cx| {
                this.preview_opening = false;
                match result {
                    Ok((handle, _)) => {
                        this.detached_preview = Some(handle);
                        this.preview_visible = false;
                    }
                    Err(error) => {
                        this.save_error = Some(format!("Could not open preview window: {error}"))
                    }
                }
                cx.notify();
            });
        });
    }
}
