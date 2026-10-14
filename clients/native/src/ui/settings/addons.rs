//! Addons snapshot of the profile (`/addons`): skill catalogs, MCP discovery
//! and package diagnostics shown by the MCP, Skills and Packages pages.

use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::addons::AppAddonsSnapshot;

use super::SettingsView;
use super::widgets::card;
use crate::ui::kit;
use crate::ui::motion::Spinner;

pub(super) enum AddonsState {
    /// Holds the pending request, so a newer one drops it.
    Loading {
        _request: Task<()>,
    },
    Failed(SharedString),
    Ready(Box<AppAddonsSnapshot>),
}

impl SettingsView {
    /// Reads the snapshot again; a newer request replaces a pending one.
    pub(super) fn load_addons(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.addons(&dir) })
                .await;
            this.update(cx, |this, cx| {
                this.addons = match result {
                    Ok(snapshot) => AddonsState::Ready(Box::new(snapshot)),
                    Err(error) => AddonsState::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .ok();
        });
        self.addons = AddonsState::Loading { _request: task };
    }

    pub(super) fn addons(&self) -> Option<&AppAddonsSnapshot> {
        match &self.addons {
            AddonsState::Ready(snapshot) => Some(snapshot),
            _ => None,
        }
    }

    /// Card for a page that waits for the snapshot or could not read it.
    pub(super) fn addons_notice(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let row = match &self.addons {
            AddonsState::Ready(_) => return None,
            AddonsState::Loading { .. } => h_flex()
                .px_4()
                .py_3()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(Spinner::new())
                .child("Проверяю навыки, MCP-серверы и пакеты…"),
            AddonsState::Failed(error) => h_flex()
                .px_4()
                .py_3()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.clone()),
                )
                .child(
                    kit::button("addons-retry")
                        .label("Повторить")
                        .outline()
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_addons(cx);
                            cx.notify();
                        })),
                ),
        };
        Some(card([row.into_any_element()], cx).into_any_element())
    }
}
