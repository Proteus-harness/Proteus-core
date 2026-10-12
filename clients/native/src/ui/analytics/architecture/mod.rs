//! «Архитектура»: build of the active chat — slot catalog, tools and the
//! interactive graph map. Topology needs a resumed session, so this page
//! follows the active chat instead of the session picker.

mod catalog;
mod map;

use std::path::PathBuf;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::topology::TopologySnapshot;

use super::widgets::{
    Fetch, Tone, badge, card, error_line, header, metric, note, refresh_button, segments,
};
use crate::api::Api;
use crate::ui::kit;
use crate::ui::kit::Hint as _;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Map,
    Catalog,
}

pub(super) struct ArchitecturePage {
    api: Api,
    session_dir: PathBuf,
    data: Fetch<TopologySnapshot>,
    tab: Tab,
    catalog: catalog::CatalogState,
    map: map::MapState,
    notice: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ArchitecturePage {
    pub fn new(
        api: Api,
        session_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tool_query = cx.new(|cx| InputState::new(window, cx).placeholder("Найти инструмент"));
        let map_query = cx.new(|cx| InputState::new(window, cx).placeholder("Найти на карте…"));
        let notify =
            |_: &mut Self, _: Entity<InputState>, event: &InputEvent, cx: &mut Context<Self>| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            };
        let subscriptions = vec![
            cx.subscribe(&tool_query, notify),
            cx.subscribe(&map_query, notify),
        ];
        Self {
            api,
            session_dir,
            data: Fetch::default(),
            tab: Tab::Map,
            catalog: catalog::CatalogState::new(tool_query),
            map: map::MapState::new(map_query),
            notice: None,
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        self.notice = None;
        self.data
            .start(move || api.topology(&dir), |this| &mut this.data, cx);
    }

    fn copy_mermaid(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        let task = cx.background_spawn(async move { api.topology_mermaid(&dir) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        "Mermaid скопирован".into()
                    }
                    Err(error) => format!("Не удалось получить Mermaid: {error:#}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for ArchitecturePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.data.stale(None) {
            self.refresh(cx);
        }
        let status = match &self.data.data {
            Some(snapshot) => format!(
                "{} слотов · {} инструментов · {} модулей · {} предупреждений",
                snapshot.slots.len(),
                snapshot.tools.iter().filter(|tool| tool.registered).count(),
                snapshot.modules.len(),
                snapshot.warnings.len()
            ),
            None if self.data.loading => "Загружаю активную сборку…".into(),
            None => "Нет данных о сборке".into(),
        };
        let mermaid = kit::button("architecture-mermaid")
            .label("Mermaid")
            .ghost()
            .small()
            .hint("Копировать как Mermaid", cx)
            .on_click(cx.listener(|this, _, _, cx| this.copy_mermaid(cx)))
            .into_any_element();
        let mut page = v_flex().size_full().gap_3().child(header(
            "Архитектура сборки",
            status,
            [
                mermaid,
                refresh_button("architecture-refresh", self.data.loading, cx, Self::refresh),
            ],
            cx,
        ));
        if let Some(notice) = &self.notice {
            page = page.child(note(notice.clone(), cx));
        }
        if let Some(error) = &self.data.error {
            page = page.child(error_line(error.clone(), cx));
        }
        let Some(snapshot) = self.data.data.clone() else {
            return page;
        };
        let hooks = if snapshot.hooks.is_empty() {
            "Отключены".to_string()
        } else {
            snapshot.hooks.join(" → ")
        };
        let model = snapshot
            .model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider, model.name))
            .unwrap_or_else(|| "Модель не выбрана".into());
        let unregistered = snapshot
            .tools
            .iter()
            .filter(|tool| !tool.registered)
            .count();
        page = page
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(metric(
                        "Профиль",
                        if snapshot.profile.is_empty() {
                            "default".to_string()
                        } else {
                            snapshot.profile.clone()
                        },
                        None,
                        cx,
                    ))
                    .child(metric("Модель", model, None, cx))
                    .child(metric("Hooks · порядок", hooks, None, cx))
                    .child(metric(
                        "Состав",
                        format!(
                            "{} модулей · {} инструментов",
                            snapshot.modules.len(),
                            snapshot.tools.len()
                        ),
                        None,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(segments(
                        "architecture-tabs",
                        &["Карта связей", "Каталог сборки"],
                        match self.tab {
                            Tab::Map => 0,
                            Tab::Catalog => 1,
                        },
                        cx,
                        |this, index, cx| {
                            this.tab = if index == 0 { Tab::Map } else { Tab::Catalog };
                            cx.notify();
                        },
                    ))
                    .child(note(
                        format!(
                            "{} предупреждений · {unregistered} незарегистрированных инструментов",
                            snapshot.warnings.len()
                        ),
                        cx,
                    )),
            );
        if !snapshot.warnings.is_empty() {
            page = page.child(
                card("Предупреждения", cx).children(snapshot.warnings.iter().map(|warning| {
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(badge(
                            warning.severity.clone(),
                            if warning.severity == "error" {
                                Tone::Danger
                            } else {
                                Tone::Warning
                            },
                            cx,
                        ))
                        .child(warning.message.clone())
                })),
            );
        }
        page.child(match self.tab {
            Tab::Map => self.render_map(&snapshot, window, cx),
            Tab::Catalog => self.render_catalog(&snapshot, cx),
        })
    }
}
