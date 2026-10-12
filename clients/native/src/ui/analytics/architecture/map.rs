//! Interactive graph map: pan by dragging, zoom with the wheel, select a
//! node to dim unrelated ones and see its facts and connections.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::topology::TopologySnapshot;

use super::super::widgets::{code, kv, note};
use super::ArchitecturePage;
use crate::analytics::format::pretty;
use crate::analytics::graph::{
    Graph, Kind, Layout, NODE_HEIGHT, NODE_WIDTH, Scope, edge_label, graph, layout,
};
use crate::ui::kit;
use crate::ui::prefs;

const MIN_ZOOM: f32 = 0.12;
const MAX_ZOOM: f32 = 2.5;

pub(super) struct MapState {
    scope: Scope,
    inactive: bool,
    selected: Option<String>,
    query: Entity<InputState>,
    offset: Point<Pixels>,
    zoom: f32,
    drag: Option<Point<Pixels>>,
    area: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Fit the map into the view on the next render with a known size.
    fit: bool,
}

impl MapState {
    pub fn new(query: Entity<InputState>) -> Self {
        Self {
            scope: Scope::Assembly,
            inactive: false,
            selected: None,
            query,
            offset: Point::default(),
            zoom: 1.,
            drag: None,
            area: Rc::default(),
            fit: true,
        }
    }

    fn fit_to(&mut self, layout: &Layout) {
        let Some(area) = self.area.get() else {
            return;
        };
        let width = f32::from(area.size.width);
        let height = f32::from(area.size.height);
        self.zoom = (width / layout.width)
            .min(height / layout.height)
            .clamp(MIN_ZOOM, 1.5);
        self.offset = point(
            px((width - layout.width * self.zoom) / 2.),
            px((height - layout.height * self.zoom) / 2.),
        );
        self.fit = false;
    }

    /// Zooms keeping the map point under `anchor` (window coordinates) still.
    fn zoom_at(&mut self, factor: f32, anchor: Option<Point<Pixels>>) {
        let Some(area) = self.area.get() else {
            return;
        };
        let local = anchor
            .map(|anchor| anchor - area.origin)
            .unwrap_or_else(|| point(area.size.width / 2., area.size.height / 2.));
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let scale = zoom / self.zoom;
        self.offset = point(
            local.x - (local.x - self.offset.x) * scale,
            local.y - (local.y - self.offset.y) * scale,
        );
        self.zoom = zoom;
    }
}

impl ArchitecturePage {
    pub(super) fn render_map(
        &mut self,
        snapshot: &TopologySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let graph = graph(snapshot);
        let layout = layout(&graph, self.map.scope, self.map.inactive);
        if self.map.fit {
            self.map.fit_to(&layout);
            if self.map.fit {
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
        }
        let query = self.map.query.read(cx).value().trim().to_string();
        let found: Vec<usize> = if query.is_empty() {
            Vec::new()
        } else {
            layout
                .nodes
                .iter()
                .filter(|placed| graph.nodes[placed.node].matches(&query))
                .map(|placed| placed.node)
                .collect()
        };
        let related = self.map.selected.as_ref().map(|id| graph.related(id));
        let theme = cx.theme().clone();

        let toolbar = h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(super::super::widgets::segments(
                "architecture-scope",
                &Scope::ALL.map(Scope::label),
                Scope::ALL
                    .iter()
                    .position(|scope| *scope == self.map.scope)
                    .unwrap_or(0),
                cx,
                |this, index, cx| {
                    this.map.scope = Scope::ALL[index];
                    this.map.fit = true;
                    cx.notify();
                },
            ))
            .when(self.map.scope != Scope::Assembly, |bar| {
                bar.child(
                    kit::checkbox("architecture-inactive")
                        .label("Неактивные")
                        .checked(self.map.inactive)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.map.inactive = *checked;
                            this.map.fit = true;
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .w(prefs::scaled(220.))
                    .child(Input::new(&self.map.query).small().cleanable(true)),
            )
            .when(!query.is_empty(), |bar| {
                bar.child(note(
                    if found.is_empty() {
                        "Ничего не найдено".to_string()
                    } else {
                        format!("Найдено: {}", found.len())
                    },
                    cx,
                ))
            })
            .child(div().flex_1())
            .child(
                kit::button("map-zoom-out")
                    .icon(IconName::Minus)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.map.zoom_at(1. / 1.2, None);
                        cx.notify();
                    })),
            )
            .child(note(format!("{:.0}%", self.map.zoom * 100.), cx))
            .child(
                kit::button("map-zoom-in")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.map.zoom_at(1.2, None);
                        cx.notify();
                    })),
            )
            .child(
                kit::button("map-fit")
                    .label("Вписать")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.map.fit = true;
                        cx.notify();
                    })),
            );

        let zoom = self.map.zoom;
        let offset = self.map.offset;
        let lines: Vec<(Point<f32>, Point<f32>, bool)> = layout
            .edges
            .iter()
            .filter_map(|index| {
                let edge = &graph.edges[*index];
                let from = layout.position(&graph, &edge.from)?;
                let to = layout.position(&graph, &edge.to)?;
                let lit = related.as_ref().is_some_and(|related| {
                    related.contains(&edge.from) && related.contains(&edge.to)
                }) && self
                    .map
                    .selected
                    .as_ref()
                    .is_some_and(|id| *id == edge.from || *id == edge.to);
                Some((point(from.x, from.y), point(to.x, to.y), lit))
            })
            .collect();
        let dimmed = related.is_some();
        let (line, lit_line) = (theme.border, theme.primary);
        let edges = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let origin = bounds.origin + offset;
                for (from, to, _) in lines.iter().filter(|(_, _, lit)| !lit) {
                    let color = if dimmed { line.opacity(0.35) } else { line };
                    paint_edge(window, origin, zoom, *from, *to, color);
                }
                for (from, to, _) in lines.iter().filter(|(_, _, lit)| *lit) {
                    paint_edge(window, origin, zoom, *from, *to, lit_line);
                }
            },
        )
        .absolute()
        .size_full();
        let area = self.map.area.clone();
        let probe = canvas(move |bounds, _, _| area.set(Some(bounds)), |_, _, _, _| {})
            .absolute()
            .size_full();

        let nodes = layout.nodes.iter().map(|placed| {
            let node = &graph.nodes[placed.node];
            let id = node.id.clone();
            let selected = self.map.selected.as_deref() == Some(node.id.as_str());
            let faded = related
                .as_ref()
                .is_some_and(|related| !related.contains(&node.id));
            let hit = found.contains(&placed.node);
            let accent = match node.kind {
                Kind::Config => theme.primary,
                Kind::Tools | Kind::Tool => theme.success,
                Kind::Module => theme.info,
                Kind::Slot => theme.warning,
                Kind::Other => theme.muted_foreground,
            };
            h_flex()
                .id(SharedString::from(format!("map-node-{}", node.id)))
                .absolute()
                .left(offset.x + px(placed.x * zoom))
                .top(offset.y + px(placed.y * zoom))
                .w(px(NODE_WIDTH * zoom))
                .h(px(NODE_HEIGHT * zoom))
                .overflow_hidden()
                .rounded(px(6. * zoom))
                .border(px(if selected || hit { 2. } else { 1. }))
                .border_color(if node.missing {
                    theme.danger
                } else if selected || hit {
                    theme.primary
                } else {
                    theme.border
                })
                .bg(theme.background)
                .cursor_pointer()
                .when(!node.active, |node| node.opacity(0.65))
                .when(faded, |node| node.opacity(0.25))
                .child(div().w(px(4. * zoom)).h_full().flex_none().bg(accent))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .px(px(8. * zoom))
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(12. * zoom))
                                .font_semibold()
                                .truncate()
                                .child(node.label.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10. * zoom))
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(node.subtitle.clone()),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.map.selected = if this.map.selected.as_deref() == Some(id.as_str()) {
                        None
                    } else {
                        Some(id.clone())
                    };
                    cx.notify();
                }))
        });

        let canvas_area = div()
            .id("architecture-map")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .cursor(if self.map.drag.is_some() {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .child(probe)
            .child(edges)
            .children(nodes)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.map.drag = Some(event.position);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some(last) = this.map.drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.map.drag = None;
                    return;
                }
                this.map.offset += event.position - last;
                this.map.drag = Some(event.position);
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.map.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(20.)).y);
                if delta != 0. {
                    this.map
                        .zoom_at((1. + delta / 400.).clamp(0.5, 2.), Some(event.position));
                    cx.notify();
                }
            }));

        let details = self
            .map
            .selected
            .as_ref()
            .and_then(|id| graph.node(id).map(|node| (id.clone(), node)))
            .map(|(id, node)| self.render_details(&graph, &id, node, cx));

        v_flex()
            .flex_1()
            .min_h(prefs::scaled(480.))
            .gap_2()
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .child(canvas_area)
                    .children(details),
            )
            .child(note(
                if self.map.selected.is_some() {
                    "Нажмите узел ещё раз, чтобы снять выбор.".to_string()
                } else {
                    format!(
                        "Выберите объект · {} объектов · {} связей · колесо — масштаб, перетаскивание — сдвиг",
                        layout.nodes.len(),
                        layout.edges.len()
                    )
                },
                cx,
            ))
            .into_any_element()
    }

    fn render_details(
        &self,
        graph: &Graph,
        id: &str,
        node: &crate::analytics::graph::Node,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let links = graph.edges.iter().filter_map(|edge| {
            let (arrow, other) = if edge.from == id {
                ("→", &edge.to)
            } else if edge.to == id {
                ("←", &edge.from)
            } else {
                return None;
            };
            let target = other.clone();
            let label = graph
                .node(other)
                .map(|node| node.label.clone())
                .unwrap_or_else(|| other.clone());
            Some(
                div()
                    .id(SharedString::from(format!("map-link-{id}-{arrow}-{other}")))
                    .text_sm()
                    .cursor_pointer()
                    .hover(|link| link.text_color(theme.primary))
                    .child(format!("{arrow} {label} · {}", edge_label(edge)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.map.selected = Some(target.clone());
                        cx.notify();
                    })),
            )
        });
        let count = graph
            .edges
            .iter()
            .filter(|edge| edge.from == id || edge.to == id)
            .count();
        v_flex()
            .id("architecture-details")
            .w(prefs::scaled(300.))
            .flex_none()
            .h_full()
            .p_3()
            .gap_2()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(node.kind.label()),
            )
            .child(div().text_lg().font_semibold().child(node.label.clone()))
            .child(note(node.subtitle.clone(), cx))
            .when(!node.description.is_empty(), |panel| {
                panel.child(div().text_sm().child(node.description.clone()))
            })
            .children(
                node.facts
                    .iter()
                    .map(|(key, value)| kv(key.clone(), value.clone(), cx)),
            )
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .child(format!("Связи · {count}")),
            )
            .children(links)
            .children(node.schema.as_ref().map(|schema| {
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child("Параметры инструмента"),
                    )
                    .child(code("map-schema", pretty(schema), cx))
            }))
            .overflow_y_scrollbar()
            .into_any_element()
    }
}

/// Bezier from the source's right side to the target's left side, or top to
/// bottom when both sit in one column.
fn paint_edge(
    window: &mut Window,
    origin: Point<Pixels>,
    zoom: f32,
    from: Point<f32>,
    to: Point<f32>,
    color: Hsla,
) {
    let at = |x: f32, y: f32| point(origin.x + px(x * zoom), origin.y + px(y * zoom));
    let (start, end, control_a, control_b) = if (to.x - from.x).abs() < 1. {
        let (top, bottom) = if from.y < to.y {
            (from, to)
        } else {
            (to, from)
        };
        let start = (top.x + NODE_WIDTH / 2., top.y + NODE_HEIGHT);
        let end = (bottom.x + NODE_WIDTH / 2., bottom.y);
        let bend = ((end.1 - start.1) / 2.).max(20.);
        (
            start,
            end,
            (start.0 + 30., start.1 + bend),
            (end.0 + 30., end.1 - bend),
        )
    } else {
        let forward = to.x > from.x;
        let start = if forward {
            (from.x + NODE_WIDTH, from.y + NODE_HEIGHT / 2.)
        } else {
            (from.x, from.y + NODE_HEIGHT / 2.)
        };
        let end = if forward {
            (to.x, to.y + NODE_HEIGHT / 2.)
        } else {
            (to.x + NODE_WIDTH, to.y + NODE_HEIGHT / 2.)
        };
        let bend = ((end.0 - start.0).abs() / 2.).max(40.);
        let sign = if forward { 1. } else { -1. };
        (
            start,
            end,
            (start.0 + bend * sign, start.1),
            (end.0 - bend * sign, end.1),
        )
    };
    let mut path = PathBuilder::stroke(px(1.5));
    path.move_to(at(start.0, start.1));
    path.cubic_bezier_to(
        at(end.0, end.1),
        at(control_a.0, control_a.1),
        at(control_b.0, control_b.1),
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
