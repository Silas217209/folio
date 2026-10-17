use super::*;
use crate::{PreviewFitPage, PreviewFitWidth, PreviewZoomIn, PreviewZoomOut, PreviewZoomReset};
use gpui_kit::component::{
    button::{ButtonCustomVariant, ButtonRounded},
    menu::{DropdownMenu, PopupMenuItem},
};

const PRESETS: &[u16] = &[25, 50, 75, 100, 200, 300, 400, 800, 1600];

impl Workspace {
    pub(super) fn zoom_out(&mut self, _: &PreviewZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom / 1.25, None, cx);
    }
    pub(super) fn zoom_in(&mut self, _: &PreviewZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom * 1.25, None, cx);
    }
    pub(super) fn zoom_reset(
        &mut self,
        _: &PreviewZoomReset,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_zoom(1., None, cx);
    }
    pub(super) fn fit_width(
        &mut self,
        _: &PreviewFitWidth,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_fit(FitMode::Width, cx);
    }
    pub(super) fn fit_page(&mut self, _: &PreviewFitPage, _: &mut Window, cx: &mut Context<Self>) {
        self.set_fit(FitMode::Page, cx);
    }
    pub(super) fn zoom_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = cx.weak_entity();
        let zoom = self.zoom;
        let fit = self.preview_fit;
        let description = match fit {
            FitMode::Manual => "Preview zoom",
            FitMode::Width => "Fit to width",
            FitMode::Page => "Fit to page",
        };
        let border: Hsla = rgb(0xd0d3d8).into();
        let button_style = ButtonCustomVariant::new(cx)
            .color(rgb(0xffffff).into())
            .foreground(rgb(0x50545b).into())
            .hover(rgb(0xf0f1f3).into())
            .active(rgb(0xe4e6e9).into());
        h_flex()
            .gap_0()
            .bg(rgb(0xffffff))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .child(
                Button::new("zoom-out")
                    .custom(button_style)
                    .small()
                    .rounded(ButtonRounded::None)
                    .w(px(28.))
                    .icon(IconName::Minus)
                    .disabled(zoom <= 0.25)
                    .tooltip(crate::runtime::shortcut("Zoom out · Ctrl−"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.zoom_out(&PreviewZoomOut, window, cx)
                    })),
            )
            .child(
                Button::new("zoom-menu")
                    .custom(button_style)
                    .small()
                    .rounded(ButtonRounded::None)
                    .w(px(80.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(border)
                    .label(format!("{}%", (zoom * 100.).round() as i32))
                    .child(Icon::new(IconName::ChevronDown).size(px(11.)))
                    .tooltip(format!("{description} · Select zoom or fit mode"))
                    .dropdown_menu(move |menu, window, _| {
                        let mut menu = menu
                            .min_w(px(208.))
                            // Natural height avoids the scroll container
                            // adding a few pixels of overflow. Keep scrolling
                            // only for genuinely short floating windows.
                            .max_h((window.viewport_size().height - px(96.)).max(px(80.)))
                            .scrollable(window.viewport_size().height < px(520.))
                            .menu_with_disabled("Zoom out", Box::new(PreviewZoomOut), zoom <= 0.25)
                            .menu_with_disabled("Zoom in", Box::new(PreviewZoomIn), zoom >= 16.)
                            .separator()
                            .menu_with_check(
                                "Fit to width",
                                fit == FitMode::Width,
                                Box::new(PreviewFitWidth),
                            )
                            .menu_with_check(
                                "Fit to page",
                                fit == FitMode::Page,
                                Box::new(PreviewFitPage),
                            )
                            .separator();
                        for &percent in PRESETS {
                            let value = f32::from(percent) / 100.;
                            let item = PopupMenuItem::new(format!("{percent}%"))
                                .checked(fit == FitMode::Manual && (zoom - value).abs() < 0.0001);
                            let item = if percent == 100 {
                                item.action(Box::new(PreviewZoomReset))
                            } else {
                                let workspace = workspace.clone();
                                item.on_click(move |_, window, cx| {
                                    let _ = workspace.update(cx, |this, cx| {
                                        this.run_command(
                                            &commands::RunCommand::new(commands::Command::Zoom {
                                                percent: value * 100.,
                                            }),
                                            window,
                                            cx,
                                        )
                                    });
                                })
                            };
                            menu = menu.item(item);
                        }
                        menu
                    }),
            )
            .child(
                Button::new("zoom-in")
                    .custom(button_style)
                    .small()
                    .rounded(ButtonRounded::None)
                    .w(px(28.))
                    .icon(IconName::Plus)
                    .disabled(zoom >= 16.)
                    .tooltip(crate::runtime::shortcut("Zoom in · Ctrl+"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.zoom_in(&PreviewZoomIn, window, cx)),
                    ),
            )
    }
}
