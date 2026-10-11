//! History page: replaced profile states and rollback into the draft.

use crate::ui::kit;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use super::widgets::{card, hint_text, note};
use super::{HistoryState, SettingsView};
use crate::settings::Draft;
use crate::settings::history::describe_changes;
use crate::ui::format::local_time;

impl SettingsView {
    pub(super) fn history_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let history = match &self.history {
            HistoryState::Idle | HistoryState::Loading => {
                return vec![
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Spinner::new().small())
                        .child("Загружаю историю…")
                        .into_any_element(),
                ];
            }
            HistoryState::Failed(error) => {
                return vec![
                    v_flex()
                        .gap_2()
                        .items_start()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(format!("Не удалось загрузить историю: {error}")),
                        )
                        .child(
                            kit::button("history-retry")
                                .label("Повторить")
                                .outline()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.load_history(cx))),
                        )
                        .into_any_element(),
                ];
            }
            HistoryState::Loaded(history) => history,
        };
        if history.revisions.is_empty() {
            return vec![
                card(
                    [note(
                        "Сохранений пока не было: история появится после первого изменения настроек агента.",
                        cx,
                    )
                    .into_any_element()],
                    cx,
                )
                .into_any_element(),
            ];
        }
        let mut newer = self.baseline.clone();
        history
            .revisions
            .iter()
            .map(|revision| {
                let before = Draft::from_revision(snapshot, &revision.state);
                let mut lines = describe_changes(snapshot, &before, &newer);
                if lines.is_empty() {
                    lines.push("Изменился только порядок значений".to_string());
                }
                let (label, enabled) = if before == self.baseline {
                    ("Текущее состояние", false)
                } else if before == self.draft {
                    ("В черновике", false)
                } else {
                    ("Вернуть как было", true)
                };
                let restore = before.clone();
                newer = before;
                let header = h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .child(div().flex_1().text_sm().font_medium().child(format!(
                        "Сохранение {}",
                        local_time(revision.replaced_at_ms / 1000)
                    )))
                    .child(
                        kit::button(ElementId::Name(format!("revision-{}", revision.id).into()))
                            .label(label)
                            .outline()
                            .xsmall()
                            .disabled(!enabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.replace_draft(restore.clone(), cx)
                            })),
                    );
                let changes = v_flex()
                    .px_4()
                    .pb_3()
                    .gap_0p5()
                    .children(lines.into_iter().filter_map(|line| hint_text(line, cx)));
                card(
                    [v_flex().child(header).child(changes).into_any_element()],
                    cx,
                )
                .into_any_element()
            })
            .collect()
    }
}
