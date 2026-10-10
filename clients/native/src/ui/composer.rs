use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppApprovalRequest;

use super::format;
use super::thread::CONTENT_WIDTH;
use super::workspace::Workspace;

const PREVIEW_LINES: usize = 24;
const PREVIEW_CHARS: usize = 6_000;

impl Workspace {
    pub(super) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let running = self.session.as_ref().is_some_and(|session| session.running());
        let ready = self
            .session
            .as_ref()
            .is_some_and(|session| session.connected);
        let action = if running {
            Button::new("stop")
                .icon(IconName::Square)
                .tooltip("Stop")
                .danger()
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("send")
                .icon(IconName::ArrowUp)
                .tooltip("Send (Enter)")
                .primary()
                .small()
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };

        h_flex().w_full().justify_center().px_6().pb_4().pt_2().child(
            h_flex()
                .w_full()
                .max_w(px(CONTENT_WIDTH))
                .items_end()
                .gap_2()
                .p_2()
                .rounded(theme.radius * 3.)
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(
                    div().flex_1().min_w_0().child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .bordered(false),
                    ),
                )
                .child(action),
        )
    }

    pub(super) fn render_approvals(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let approvals = &self.session.as_ref()?.pending.as_ref()?.approvals;
        if approvals.is_empty() {
            return None;
        }
        let cards = approvals
            .iter()
            .map(|request| self.render_approval(request, cx))
            .collect::<Vec<_>>();
        Some(
            h_flex().w_full().justify_center().px_6().pt_2().child(
                v_flex()
                    .w_full()
                    .max_w(px(CONTENT_WIDTH))
                    .gap_2()
                    .children(cards),
            ),
        )
    }

    fn render_approval(&self, request: &AppApprovalRequest, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let preview = request.preview.as_ref();
        let title = preview
            .map(|preview| preview.title.clone())
            .unwrap_or_else(|| request.call.name.clone());
        let summary = preview
            .map(|preview| preview.summary.clone())
            .filter(|summary| !summary.trim().is_empty());
        let body = preview
            .and_then(|preview| preview.body.as_deref())
            .or_else(|| {
                (preview.is_none() && !request.call.args.is_null()).then_some("")
            })
            .map(|body| {
                let text = if body.is_empty() {
                    serde_json::to_string_pretty(&request.call.args).unwrap_or_default()
                } else {
                    body.to_owned()
                };
                format::clip_block(&text, PREVIEW_LINES, PREVIEW_CHARS)
            });

        let approve_id = request.approval_id.clone();
        let deny_id = request.approval_id.clone();
        v_flex()
            .p_3()
            .gap_2()
            .rounded(theme.radius * 2.)
            .border_1()
            .border_color(theme.warning)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(Icon::new(IconName::TriangleAlert).small().text_color(theme.warning))
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
                    .id(SharedString::from(format!("approval-body-{}", request.approval_id)))
                    .max_h(px(280.))
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
                        Button::new(SharedString::from(format!("deny-{deny_id}")))
                            .label("Deny")
                            .outline()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resolve_approval(deny_id.clone(), false, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("approve-{approve_id}")))
                            .label("Approve")
                            .primary()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resolve_approval(approve_id.clone(), true, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}
