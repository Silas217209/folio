//! App prompts use the same components and observable buttons as the editor.
use gpui_kit::{
    component::{
        ActiveTheme,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    prelude::*,
    *,
};
struct PromptView {
    message: String,
    detail: Option<String>,
    answers: Vec<SharedString>,
    focus: FocusHandle,
}
impl EventEmitter<PromptResponse> for PromptView {}
impl Focusable for PromptView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for PromptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x18241c55))
            .occlude()
            .child(
                v_flex()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        let answer = match event.keystroke.key.as_str() {
                            "escape" => Some(this.answers.len().saturating_sub(1)),
                            "enter" => Some(0),
                            _ => None,
                        };
                        if let Some(answer) = answer {
                            cx.emit(PromptResponse(answer));
                            cx.stop_propagation();
                        }
                    }))
                    .w(px(440.))
                    .p_6()
                    .gap_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .shadow_lg()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.message.clone()),
                    )
                    .when_some(self.detail.clone(), |view, detail| {
                        view.child(
                            div()
                                .text_size(px(13.))
                                .text_color(cx.theme().muted_foreground)
                                .child(detail),
                        )
                    })
                    .child(h_flex().gap_2().justify_end().children(
                        self.answers.iter().enumerate().map(|(index, label)| {
                            Button::new(("prompt-answer", index))
                                .label(label.clone())
                                .when(index == 0, |button| button.primary())
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(PromptResponse(index));
                                    cx.stop_propagation();
                                }))
                        }),
                    )),
            )
    }
}
pub fn init(cx: &mut App) {
    cx.set_prompt_builder(|_, message, detail, answers, handle, window, cx| {
        let view = cx.new(|cx| PromptView {
            message: message.into(),
            detail: detail.map(str::to_owned),
            answers: answers
                .iter()
                .map(|answer| answer.label().clone())
                .collect(),
            focus: cx.focus_handle(),
        });
        handle.with_view(view, window, cx)
    });
}
