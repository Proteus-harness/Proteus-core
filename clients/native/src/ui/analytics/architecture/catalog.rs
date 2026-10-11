//! Build catalog: slots with their active module and alternatives, then tools.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::topology::{ToolTopology, TopologySnapshot};

use super::super::widgets::{Tone, badge, card, code, note};
use super::ArchitecturePage;
use crate::analytics::format::pretty;
use crate::analytics::graph::slot_views;
use crate::ui::prefs;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolFilter {
    All,
    Enabled,
    Disabled,
    ReadOnly,
    Mutating,
    Process,
}

impl ToolFilter {
    const ALL: [Self; 6] = [
        Self::All,
        Self::Enabled,
        Self::Disabled,
        Self::ReadOnly,
        Self::Mutating,
        Self::Process,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "Все",
            Self::Enabled => "Включены",
            Self::Disabled => "Отключены",
            Self::ReadOnly => "Чтение",
            Self::Mutating => "Изменение",
            Self::Process => "Процессные",
        }
    }

    fn keeps(self, tool: &ToolTopology) -> bool {
        match self {
            Self::All => true,
            Self::Enabled => tool.enabled && tool.registered,
            Self::Disabled => !(tool.enabled && tool.registered),
            Self::ReadOnly => tool.safety == "ReadOnly",
            Self::Mutating => tool.safety != "ReadOnly",
            Self::Process => tool.source.contains("process-module"),
        }
    }
}

pub(super) struct CatalogState {
    filter: ToolFilter,
    query: Entity<InputState>,
    schema: Option<String>,
}

impl CatalogState {
    pub fn new(query: Entity<InputState>) -> Self {
        Self {
            filter: ToolFilter::All,
            query,
            schema: None,
        }
    }
}

impl ArchitecturePage {
    pub(super) fn render_catalog(
        &self,
        snapshot: &TopologySnapshot,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let slots = slot_views(snapshot).into_iter().map(|view| {
            let status = match (&view.active, view.slot.required) {
                (Some(_), _) => badge("Активен", Tone::Success, cx),
                (None, true) => badge("Не выбран", Tone::Danger, cx),
                (None, false) => badge("Отключён", Tone::Muted, cx),
            };
            let description = view
                .active
                .as_ref()
                .and_then(|module| module.description.clone())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| view.slot.responsibility.clone());
            card(view.slot.id.clone(), cx)
                .min_w(prefs::scaled(260.))
                .flex_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div().font_semibold().child(
                                view.active
                                    .as_ref()
                                    .map(|module| module.id.clone())
                                    .unwrap_or_else(|| "Модуль не выбран".into()),
                            ),
                        )
                        .children(
                            view.active
                                .as_ref()
                                .map(|module| badge(module.source.kind(), Tone::Muted, cx)),
                        ),
                )
                .child(note(description, cx))
                .child(h_flex().gap_1().child(status).child(badge(
                    if view.slot.required {
                        "Обязательный"
                    } else {
                        "Необязательный"
                    },
                    Tone::Muted,
                    cx,
                )))
                .when(!view.alternatives.is_empty(), |card| {
                    card.child(note(
                        format!("Другие реализации: {}", view.alternatives.len()),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .children(view.alternatives.iter().map(|module| {
                                badge(
                                    format!("{} · {}", module.id, module.source.kind()),
                                    Tone::Muted,
                                    cx,
                                )
                            })),
                    )
                })
        });
        let query = self.catalog.query.read(cx).value().trim().to_lowercase();
        let tools = snapshot.tools.iter().filter(|tool| {
            self.catalog.filter.keeps(tool)
                && (query.is_empty() || tool.name.to_lowercase().contains(&query))
        });
        let filters = h_flex()
            .gap_1()
            .children(ToolFilter::ALL.iter().map(|filter| {
                let filter = *filter;
                let active = filter == self.catalog.filter;
                div()
                    .id(filter.label())
                    .px_2()
                    .py_0p5()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |chip| chip.bg(theme.accent).font_semibold())
                    .when(!active, |chip| chip.hover(|chip| chip.bg(theme.list_hover)))
                    .child(filter.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.catalog.filter = filter;
                        cx.notify();
                    }))
            }));
        let tool_cards = tools.map(|tool| {
            let name = tool.name.clone();
            let open = self.catalog.schema.as_deref() == Some(tool.name.as_str());
            card(tool.source.clone(), cx)
                .child(div().font_semibold().child(tool.name.clone()))
                .child(note(tool.description.clone(), cx).line_clamp(3))
                .child(
                    h_flex()
                        .gap_1()
                        .child(badge(tool.safety.clone(), Tone::Muted, cx))
                        .child(badge(
                            if tool.registered {
                                "Зарегистрирован"
                            } else {
                                "Предоставлен"
                            },
                            Tone::Muted,
                            cx,
                        ))
                        .child(if tool.enabled {
                            badge("Включён", Tone::Success, cx)
                        } else {
                            badge("Отключён", Tone::Warning, cx)
                        }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("tool-schema-{name}")))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|link| link.text_color(theme.foreground))
                        .child(if open {
                            "Скрыть схему параметров"
                        } else {
                            "Схема параметров"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.catalog.schema =
                                if this.catalog.schema.as_deref() == Some(name.as_str()) {
                                    None
                                } else {
                                    Some(name.clone())
                                };
                            cx.notify();
                        })),
                )
                .when(open, |card| {
                    card.child(code(
                        SharedString::from(format!("tool-schema-code-{}", tool.name)),
                        pretty(&tool.input_schema),
                        cx,
                    ))
                })
        });
        div()
            .id("architecture-catalog")
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_flex()
                    .gap_3()
                    .pb_4()
                    .child(div().font_semibold().child("Слоты сборки"))
                    .child(h_flex().flex_wrap().gap_3().items_start().children(slots))
                    .child(div().font_semibold().child("Инструменты"))
                    .child(
                        h_flex().gap_3().child(filters).child(
                            div()
                                .w(prefs::scaled(240.))
                                .child(Input::new(&self.catalog.query).small().cleanable(true)),
                        ),
                    )
                    .child(v_flex().gap_2().children(tool_cards)),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}
