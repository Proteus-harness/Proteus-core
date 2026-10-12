//! The model icon's chooser: the effort step on top with the model under it
//! and the effort pill below. The model opens its list in their place, and a
//! chosen model returns to the effort without closing the chooser.

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
            self.effort_header(config, chat, shown, cx)
                .into_any_element(),
            div()
                .px_1()
                .pb_1()
                .child(self.pill(count, shown, cx))
                .into_any_element(),
        ]
    }

    fn effort_header(
        &self,
        config: &ConfigSummary,
        chat: WeakEntity<Chat>,
        shown: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (muted, foreground, primary) =
            (theme.muted_foreground, theme.foreground, theme.primary);
        let title = efforts(config).get(shown).cloned().unwrap_or_default();
        let model = h_flex()
            .id("selector-model")
            .max_w_full()
            .gap_0p5()
            .text_color(muted)
            .child(div().child(faded(model_label(config))))
            .when(!models(config).is_empty(), |row| {
                row.cursor_pointer()
                    .hover(move |style| style.text_color(foreground))
                    .child(Icon::new(Lucide::ChevronRight).xsmall())
                    .on_click(cx.listener(|this, _, _, cx| this.show_models(true, cx)))
            });
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
            .relative()
            .px_8()
            .pt_1()
            .pb_2()
            .child(
                v_flex()
                    .items_center()
                    .gap_0p5()
                    .child(
                        div()
                            .text_base()
                            .font_medium()
                            .text_color(primary)
                            .child(title),
                    )
                    .child(model),
            )
            .child(div().absolute().top_1().right_1().child(reset))
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
                        .hint("К рассуждениям", cx)
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
