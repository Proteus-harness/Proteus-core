//! The model icon's chooser: the model on top and the reasoning effort with
//! its pill below. The model opens its list in their place, and a chosen
//! model returns to the effort without closing the chooser.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::config::ConfigSummary;

use super::popup::{choice, heading};
use super::{SelectorBadge, effort_position};
use crate::ui::chat::Chat;
use crate::ui::chat::controls::{current_effort, efforts, model_label, models, models_provider};
use crate::ui::fade::faded;
use crate::ui::kit::Hint as _;
use crate::ui::{kit, prefs};

impl SelectorBadge {
    pub(super) fn model_chooser(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let count = efforts(config).len();
        // Without effort steps the list is all there is to choose.
        if count < 2 {
            return self.model_list(config, chat, false, cx);
        }
        if self.models_open && !models(config).is_empty() {
            return self.model_list(config, chat, true, cx);
        }
        let shown = self
            .scrub
            .or(self.pending)
            .unwrap_or_else(|| effort_position(config))
            .min(count - 1);
        vec![
            self.model_row(config, cx).into_any_element(),
            self.effort_row(config, chat, shown, cx).into_any_element(),
            div()
                .px_1()
                .pb_1()
                .child(self.pill(count, shown, cx))
                .into_any_element(),
        ]
    }

    fn model_row(&self, config: &ConfigSummary, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, hover, radius) = (theme.muted_foreground, theme.list_hover, theme.radius);
        h_flex()
            .id("selector-model")
            .gap_1()
            .px_2()
            .py_1()
            .rounded(radius)
            .font_medium()
            .child(div().flex_1().min_w_0().child(faded(model_label(config))))
            .when(!models(config).is_empty(), |row| {
                row.cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(
                        div()
                            .text_color(muted)
                            .child(Icon::new(Lucide::ChevronRight).xsmall()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.show_models(true, cx)))
            })
    }

    fn effort_row(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        shown: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (muted, primary, border) = (theme.muted_foreground, theme.primary, theme.border);
        let title = efforts(config).get(shown).cloned().unwrap_or_default();
        let overridden = current_effort(config).is_some() || self.pending.is_some();
        let this = cx.entity().downgrade();
        let reset = kit::button("selector-effort-reset")
            .icon(Lucide::RotateCcw)
            .ghost()
            .xsmall()
            .disabled(!overridden)
            .hint("Как в профиле", cx)
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.scrub = None;
                    this.pending = None;
                    cx.notify();
                });
                let _ = chat.update(cx, |chat, cx| chat.set_effort(None, cx));
            });
        div()
            .mt_1()
            .pt_1()
            .border_t_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_1()
                    .pl_2()
                    .pr_0p5()
                    .pb_1()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(muted)
                            .child("Рассуждения"),
                    )
                    .child(div().font_medium().text_color(primary).child(title))
                    .child(reset),
            )
    }

    fn model_list(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        back: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let title: SharedString = match models_provider(config) {
            Some(provider) => format!("Модель · {provider}").into(),
            None => "Модель".into(),
        };
        let header = if back {
            h_flex()
                .gap_0p5()
                .child(
                    kit::button("selector-models-back")
                        .icon(Lucide::ChevronLeft)
                        .ghost()
                        .xsmall()
                        .hint("Назад", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.show_models(false, cx))),
                )
                .child(heading(title, cx).px_0().pt_0().pb_0())
        } else {
            h_flex().child(heading(title, cx))
        };
        let current = config.model.as_ref().map(|model| model.name.clone());
        let rows = models(config)
            .into_iter()
            .enumerate()
            .map(|(index, (name, label))| {
                let selected = current.as_ref() == Some(&name);
                let chat = chat.clone();
                let this = cx.entity().downgrade();
                choice(("model", index), label, selected, cx, move |cx| {
                    let _ = chat.update(cx, |chat, cx| chat.set_model(name.clone(), cx));
                    let _ = this.update(cx, |this, cx| {
                        this.scrub = None;
                        this.pending = None;
                        this.show_models(false, cx);
                    });
                })
            });
        vec![
            header.into_any_element(),
            v_flex()
                .id("selector-models")
                .max_h(prefs::scaled(320.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element(),
        ]
    }
}
