//! Generic interactive graph: local pan, zoom and node dragging.

use crate::declarative::graph::{Edge, Node};
use crate::ui::{fade::faded, kit, kit::Pressable as _, prefs};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{cell::Cell, collections::HashMap, rc::Rc};

const NODE_WIDTH: f32 = 180.;
const NODE_HEIGHT: f32 = 64.;

enum Drag {
    Pan(Point<Pixels>, Point<Pixels>),
    Node(String, Point<Pixels>, Point<f32>),
}

pub(super) struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    offset: Point<Pixels>,
    zoom: f32,
    positions: HashMap<String, Point<f32>>,
    drag: Option<Drag>,
    area: Rc<Cell<Option<Bounds<Pixels>>>>,
    fit: bool,
    selected: Rc<dyn Fn(String, &mut App)>,
}

impl Graph {
    pub fn new(
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        selected: impl Fn(String, &mut App) + 'static,
    ) -> Self {
        Self {
            nodes,
            edges,
            offset: point(px(0.), px(0.)),
            zoom: 1.,
            positions: HashMap::new(),
            drag: None,
            area: Rc::default(),
            fit: true,
            selected: Rc::new(selected),
        }
    }
    pub fn set_data(&mut self, nodes: Vec<Node>, edges: Vec<Edge>, cx: &mut Context<Self>) {
        if self.nodes == nodes && self.edges == edges {
            return;
        }
        self.positions
            .retain(|id, _| nodes.iter().any(|node| &node.id == id));
        self.nodes = nodes;
        self.edges = edges;
        cx.notify();
    }

    fn position(&self, node: &Node) -> Point<f32> {
        self.positions
            .get(&node.id)
            .copied()
            .unwrap_or(point(node.x, node.y))
    }
    fn zoom_at(&mut self, factor: f32, position: Option<Point<Pixels>>) {
        let Some(area) = self.area.get() else {
            return;
        };
        let at = position.unwrap_or(area.center()) - area.origin;
        let next = (self.zoom * factor).clamp(0.15, 3.);
        self.offset = at - (at - self.offset) * (next / self.zoom);
        self.zoom = next;
    }
    fn fit(&mut self) {
        let Some(area) = self.area.get() else {
            return;
        };
        let width = self
            .nodes
            .iter()
            .map(|node| self.position(node).x + NODE_WIDTH + 32.)
            .fold(1., f32::max);
        let height = self
            .nodes
            .iter()
            .map(|node| self.position(node).y + NODE_HEIGHT + 32.)
            .fold(1., f32::max);
        self.zoom = (f32::from(area.size.width) / width)
            .min(f32::from(area.size.height) / height)
            .clamp(0.15, 1.);
        self.offset = point(px(8.), px(8.));
        self.fit = false;
    }
}

impl Render for Graph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.fit {
            self.fit();
            if self.fit {
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
        }
        let zoom = self.zoom;
        let offset = self.offset;
        let theme = cx.theme();
        let (border, background, muted, primary) = (
            theme.border,
            theme.background,
            theme.muted_foreground,
            theme.primary,
        );
        let positions: HashMap<_, _> = self
            .nodes
            .iter()
            .map(|node| (node.id.clone(), self.position(node)))
            .collect();
        let lines: Vec<_> = self
            .edges
            .iter()
            .filter_map(|edge| Some((*positions.get(&edge.from)?, *positions.get(&edge.to)?)))
            .collect();
        let edges = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                for (from, to) in &lines {
                    paint_edge(window, bounds.origin + offset, zoom, *from, *to, border);
                }
            },
        )
        .absolute()
        .size_full();
        let nodes: Vec<_> = self
            .nodes
            .iter()
            .map(|node| {
                let at = self.position(node);
                let (select, drag) = (node.id.clone(), node.id.clone());
                let accent = super::render::tone_color(node.tone, cx);
                h_flex()
                    .id(SharedString::from(format!("graph-node-{}", node.id)))
                    .absolute()
                    .left(offset.x + px(at.x * zoom))
                    .top(offset.y + px(at.y * zoom))
                    .w(px(NODE_WIDTH * zoom))
                    .h(px(NODE_HEIGHT * zoom))
                    .rounded(px(6. * zoom))
                    .border_1()
                    .border_color(if node.selected { primary } else { border })
                    .bg(background)
                    .overflow_hidden()
                    .cursor_pointer()
                    .pressable(cx)
                    .when(node.dimmed, |node| node.opacity(0.4))
                    .child(div().w(px(4. * zoom)).h_full().bg(accent))
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
                                    .child(faded(node.label.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(10. * zoom))
                                    .text_color(muted)
                                    .child(faded(node.subtitle.clone())),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.drag = Some(Drag::Node(drag.clone(), event.position, at));
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (this.selected)(select.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let area = self.area.clone();
        let probe = canvas(move |bounds, _, _| area.set(Some(bounds)), |_, _, _, _| {})
            .absolute()
            .size_full();
        let graph = div()
            .id("extension-graph")
            .relative()
            .flex_1()
            .min_h(prefs::scaled(240.))
            .w_full()
            .overflow_hidden()
            .bg(background)
            .child(probe)
            .child(edges)
            .children(nodes)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.drag = Some(Drag::Pan(event.position, this.offset))
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                match &this.drag {
                    Some(Drag::Pan(start, offset)) => {
                        this.offset = *offset + (event.position - *start)
                    }
                    Some(Drag::Node(id, start, at)) => {
                        let delta = event.position - *start;
                        this.positions.insert(
                            id.clone(),
                            *at + point(
                                f32::from(delta.x) / this.zoom,
                                f32::from(delta.y) / this.zoom,
                            ),
                        );
                    }
                    None => return,
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(16.));
                this.zoom_at((f32::from(delta.y) * 0.002).exp(), Some(event.position));
                cx.notify();
                cx.stop_propagation();
            }));
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        kit::button("graph-minus")
                            .small()
                            .label("−")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_at(1. / 1.2, None);
                                cx.notify();
                            })),
                    )
                    .child(div().text_xs().child(format!("{:.0}%", zoom * 100.)))
                    .child(
                        kit::button("graph-plus")
                            .small()
                            .label("+")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_at(1.2, None);
                                cx.notify();
                            })),
                    )
                    .child(kit::button("graph-fit").small().label("Вписать").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.fit = true;
                            cx.notify();
                        }),
                    )),
            )
            .child(graph)
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
