use std::hash::{DefaultHasher, Hash, Hasher};

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::{AppApprovalRequest, AppPendingRequests};
use proteus_contracts::contracts::UserInputRequest;

use super::Chat;
use crate::ui::kit;
use crate::ui::kit::Pressable as _;
use crate::ui::prefs;
use crate::ui::{format, motion};

const PREVIEW_LINES: usize = 24;
const PREVIEW_CHARS: usize = 6_000;

impl Chat {
    /// Approval and question cards the agent is waiting on.
    pub(super) fn render_requests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let pending = self.session.pending.clone()?;
        if pending.approvals.is_empty() && pending.user_inputs.is_empty() {
            return None;
        }
        let mut cards = pending
            .approvals
            .iter()
            .map(|request| {
                let id = approval_card(request);
                let fresh = self.fresh_requests.is_fresh(request_key(&id));
                arrive(render_approval(request, cx), id, fresh)
            })
            .collect::<Vec<_>>();
        for request in &pending.user_inputs {
            let id = input_card(request);
            let fresh = self.fresh_requests.is_fresh(request_key(&id));
            cards.push(arrive(
                self.render_user_input(request, window, cx),
                id,
                fresh,
            ));
        }
        Some(
            h_flex().w_full().justify_center().px_6().pt_2().child(
                v_flex()
                    .id("requests")
                    .w_full()
                    .max_w(prefs::chat_width(cx))
                    .max_h(prefs::scaled(480.))
                    .overflow_y_scroll()
                    .gap_2()
                    .children(cards),
            ),
        )
    }

    fn render_user_input(
        &mut self,
        request: &UserInputRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let request_id = request.request_id.clone();
        let mut questions = Vec::with_capacity(request.questions.len());
        for question in &request.questions {
            let options = question
                .options
                .iter()
                .enumerate()
                .map(|(ix, option)| {
                    let selected =
                        self.user_input
                            .is_selected(&request_id, &question.id, &option.label);
                    let (rid, qid, label) = (
                        request_id.clone(),
                        question.id.clone(),
                        option.label.clone(),
                    );
                    let multi = question.multi_select;
                    h_flex()
                        .id(SharedString::from(format!(
                            "answer-{request_id}-{}-{ix}",
                            question.id
                        )))
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .pressable(cx)
                        .when(selected, |row| row.bg(theme.accent))
                        .hover(|row| row.bg(theme.list_hover))
                        .child(div().w_4().flex_none().when(selected, |slot| {
                            slot.child(Icon::new(IconName::Check).small().text_color(theme.primary))
                        }))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(div().text_sm().child(option.label.clone()))
                                .children((!option.description.trim().is_empty()).then(|| {
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(option.description.clone())
                                })),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.user_input.toggle(&rid, &qid, &label, multi);
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>();
            let free_text = (question.is_other || question.options.is_empty()).then(|| {
                let placeholder = if question.options.is_empty() {
                    "Ответ"
                } else {
                    "Свой вариант"
                };
                let state = self.user_input.text_input(
                    &request_id,
                    &question.id,
                    question.is_secret,
                    placeholder,
                    window,
                    cx,
                );
                Input::new(&state).small()
            });
            questions.push(
                v_flex()
                    .gap_1()
                    .when(!question.header.trim().is_empty(), |block| {
                        block.child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(theme.muted_foreground)
                                .child(question.header.clone()),
                        )
                    })
                    .child(div().text_sm().child(question.question.clone()))
                    .children(options)
                    .children(free_text),
            );
        }

        v_flex()
            .p_3()
            .gap_3()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.info)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(Icon::new(IconName::Info).small().text_color(theme.info))
                    .child(
                        div().font_semibold().child(
                            request
                                .title
                                .clone()
                                .unwrap_or_else(|| "Вопрос агента".to_owned()),
                        ),
                    ),
            )
            .children(questions)
            .child(
                h_flex().justify_end().child(
                    kit::button(SharedString::from(format!("answer-{request_id}")))
                        .label("Ответить")
                        .primary()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.submit_user_input(request_id.clone(), cx)
                        })),
                ),
            )
            .into_any_element()
    }
}

/// A request card rises in once, when it arrives; shown again after a tab
/// switch it stays put.
fn arrive(card: AnyElement, id: String, fresh: bool) -> AnyElement {
    motion::rise_in_if(div().child(card), ElementId::Name(id.into()), fresh).into_any_element()
}

/// Keys of the cards in `pending`, for telling the newly arrived ones.
pub(super) fn request_keys(pending: &AppPendingRequests) -> impl Iterator<Item = u64> + '_ {
    let approvals = pending.approvals.iter().map(approval_card);
    let inputs = pending.user_inputs.iter().map(input_card);
    approvals.chain(inputs).map(|id| request_key(&id))
}

fn approval_card(request: &AppApprovalRequest) -> String {
    format!("approval-{}", request.approval_id)
}

fn input_card(request: &UserInputRequest) -> String {
    format!("user-input-{}", request.request_id)
}

fn request_key(id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

fn render_approval(request: &AppApprovalRequest, cx: &mut Context<Chat>) -> AnyElement {
    let theme = cx.theme();
    let preview = request.preview.as_ref();
    let title = preview
        .map(|preview| preview.title.clone())
        .unwrap_or_else(|| request.call.name.clone());
    let summary = preview
        .map(|preview| preview.summary.clone())
        .filter(|summary| !summary.trim().is_empty());
    let body = match preview {
        Some(preview) => preview.body.clone(),
        None if !request.call.args.is_null() => {
            serde_json::to_string_pretty(&request.call.args).ok()
        }
        None => None,
    }
    .map(|body| format::clip_block(&body, PREVIEW_LINES, PREVIEW_CHARS));

    let approve_id = request.approval_id.clone();
    let deny_id = request.approval_id.clone();
    v_flex()
        .p_3()
        .gap_2()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.warning)
        .bg(theme.background)
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(theme.warning),
                )
                .child(div().font_semibold().child(title)),
        )
        .when(!request.reason.trim().is_empty(), |card| {
            card.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(request.reason.clone()),
            )
        })
        .children(summary.map(|summary| div().text_sm().child(summary)))
        .children(body.map(|body| {
            div()
                .id(SharedString::from(format!(
                    "approval-body-{}",
                    request.approval_id
                )))
                .max_h(prefs::scaled(280.))
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .rounded(theme.radius)
                .bg(theme.muted)
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(body)
        }))
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .child(
                    kit::button(SharedString::from(format!("deny-{deny_id}")))
                        .label("Отклонить")
                        .outline()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resolve_approval(deny_id.clone(), false, cx)
                        })),
                )
                .child(
                    kit::button(SharedString::from(format!("approve-{approve_id}")))
                        .label("Разрешить")
                        .primary()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resolve_approval(approve_id.clone(), true, cx)
                        })),
                ),
        )
        .into_any_element()
}
