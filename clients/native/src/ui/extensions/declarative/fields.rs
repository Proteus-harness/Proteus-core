//! Persistent native control state for process-owned widget trees.

use super::{terminal::TerminalView, tree::Tree, view::DeclarativeView};
use crate::declarative::widget::Node;
use gpui_kit::component::input::{EditorState, InputEvent, InputState};
use gpui_kit::*;
use std::collections::HashMap;

pub(super) struct TextInput {
    pub state: Entity<InputState>,
    value: String,
    _subscription: Subscription,
}

#[derive(Default)]
pub(super) struct Fields {
    pub inputs: HashMap<String, TextInput>,
    pub codes: HashMap<String, Entity<EditorState>>,
    pub terminals: HashMap<String, Entity<TerminalView>>,
    pub graphs: HashMap<String, Entity<super::graph::Graph>>,
    pub trees: HashMap<String, Entity<Tree>>,
}

impl DeclarativeView {
    pub(super) fn prune_fields(&mut self, root: &Node) {
        let mut ids = std::collections::HashSet::new();
        root.visit(&mut |node| match node {
            Node::Input { id, .. } => {
                ids.insert((0, id.clone()));
            }
            Node::Code { id, .. } => {
                ids.insert((1, id.clone()));
            }
            Node::Terminal { id, .. } => {
                ids.insert((2, id.clone()));
            }
            Node::Tree { id, .. } => {
                ids.insert((3, id.clone()));
            }
            Node::Graph { id, .. } => {
                ids.insert((4, id.clone()));
            }
            Node::Steps { id, .. } => {
                ids.insert((5, id.clone()));
            }
            _ => {}
        });
        self.fields
            .inputs
            .retain(|id, _| ids.contains(&(0, id.clone())));
        self.fields
            .codes
            .retain(|id, _| ids.contains(&(1, id.clone())));
        self.fields
            .terminals
            .retain(|id, _| ids.contains(&(2, id.clone())));
        self.fields
            .trees
            .retain(|id, _| ids.contains(&(3, id.clone())));
        self.fields
            .graphs
            .retain(|id, _| ids.contains(&(4, id.clone())));
        self.steps.retain(|id, _| ids.contains(&(5, id.clone())));
    }

    pub(super) fn prepare_fields(
        &mut self,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match node {
            Node::Graph {
                id,
                nodes,
                edges,
                action,
            } => {
                if let Some(graph) = self.fields.graphs.get(id) {
                    graph.update(cx, |graph, cx| {
                        graph.set_data(nodes.clone(), edges.clone(), cx)
                    });
                } else {
                    let (this, action) = (cx.weak_entity(), action.clone());
                    let graph = cx.new(|_| {
                        super::graph::Graph::new(nodes.clone(), edges.clone(), move |id, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.act(action.clone(), Some(serde_json::json!(id)), cx)
                            });
                        })
                    });
                    self.fields.graphs.insert(id.clone(), graph);
                }
            }
            Node::Input {
                id,
                value,
                placeholder,
                action,
            } => {
                if let Some(input) = self.fields.inputs.get_mut(id) {
                    if input.value != *value {
                        input.value = value.clone();
                        if input.state.read(cx).value().as_ref() != value {
                            input
                                .state
                                .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                        }
                    }
                } else {
                    let state = cx.new(|cx| {
                        let mut input =
                            InputState::new(window, cx).placeholder(placeholder.clone());
                        input.set_value(value.clone(), window, cx);
                        input
                    });
                    let (action, identity) = (action.clone(), id.clone());
                    let subscription =
                        cx.subscribe(&state, move |this, state, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::Change)
                                && this.fields.inputs.get(&identity).is_some_and(|input| {
                                    input.value != state.read(cx).value().as_ref()
                                })
                            {
                                this.act(
                                    action.clone(),
                                    Some(serde_json::json!(state.read(cx).value().to_string())),
                                    cx,
                                );
                            }
                        });
                    self.fields.inputs.insert(
                        id.clone(),
                        TextInput {
                            state,
                            value: value.clone(),
                            _subscription: subscription,
                        },
                    );
                }
            }
            Node::Code {
                id, text, language, ..
            } => {
                let editor = self.fields.codes.entry(id.clone()).or_insert_with(|| {
                    cx.new(|cx| {
                        let mut editor = EditorState::new(window, cx)
                            .language(language.clone())
                            .line_number(true)
                            .soft_wrap(false);
                        editor.set_readonly(true, cx);
                        editor
                    })
                });
                editor.update(cx, |editor, cx| {
                    editor.set_highlighter(language.clone(), cx);
                    if editor.value().as_ref() != text {
                        editor.set_value(text.clone(), window, cx);
                    }
                });
            }
            Node::Terminal { id, cwd } => {
                if !self.fields.terminals.contains_key(id) {
                    let terminal = cx.new(|cx| TerminalView::new(cwd.clone(), window, cx));
                    let focus = self.focus.clone();
                    let target = terminal.read(cx).focus_handle(cx);
                    cx.defer_in(window, move |_, window, cx| {
                        if focus.is_focused(window) {
                            target.focus(window, cx);
                        }
                    });
                    self.fields.terminals.insert(id.clone(), terminal);
                }
            }
            Node::Tree { id, rows, action } => {
                if let Some(tree) = self.fields.trees.get(id) {
                    tree.update(cx, |tree, cx| tree.set_rows(rows.clone(), cx));
                } else {
                    let (this, action) = (cx.weak_entity(), action.clone());
                    let tree = cx.new(|cx| {
                        Tree::new(rows.clone(), cx, move |id, event, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.act(
                                    action.clone(),
                                    Some(serde_json::json!({"id":id,"event":event})),
                                    cx,
                                )
                            });
                        })
                    });
                    self.fields.trees.insert(id.clone(), tree);
                }
            }
            Node::Stack { children, .. }
            | Node::Card { children, .. }
            | Node::Fill { children } => {
                for child in children {
                    self.prepare_fields(child, window, cx);
                }
            }
            Node::Tabs { tabs } => {
                for tab in tabs {
                    for child in &tab.children {
                        self.prepare_fields(child, window, cx);
                    }
                }
            }
            _ => {}
        }
    }
}
