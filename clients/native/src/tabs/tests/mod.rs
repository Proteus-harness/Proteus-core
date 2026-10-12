use std::path::Path;

use super::{Node, PaneId, Size, TabLayout, chat, chat_dir, split_document};

mod pins;
mod sizes;
mod tree;

const A: &str = "chat:/s/a";
const B: &str = "chat:/s/b";

/// Compact picture of the tree: `0[a, b*]` is pane 0 showing `b`, `^`
/// marks pinned tabs; `row(…)` and `col(…)` are splits,
/// `264px ` marks a fixed child.
fn shape(layout: &TabLayout) -> String {
    fn node(tree: &Node) -> String {
        match tree {
            Node::Pane(pane) => {
                let tabs: Vec<String> = pane
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(ix, tab)| {
                        let pin = if ix < pane.pinned { "^" } else { "" };
                        let mark = if pane.active.as_ref() == Some(tab) {
                            "*"
                        } else {
                            ""
                        };
                        let name = tab.strip_prefix("chat:/s/").unwrap_or(tab);
                        format!("{name}{pin}{mark}")
                    })
                    .collect();
                format!("{}[{}]", pane.id.0, tabs.join(", "))
            }
            Node::Split(split) => {
                let (name, sep) = match split.axis {
                    super::Axis::Row => ("row", " | "),
                    super::Axis::Column => ("col", " / "),
                };
                let children: Vec<String> = split
                    .children
                    .iter()
                    .map(|child| match child.size {
                        Size::Fixed(px) => format!("{px}px {}", node(&child.node)),
                        Size::Weight(_) => node(&child.node),
                    })
                    .collect();
                format!("{name}({})", children.join(sep))
            }
        }
    }
    assert_eq!(layout.validate(), Ok(()));
    node(&layout.root)
}

/// One empty pane.
fn blank() -> TabLayout {
    TabLayout::default()
}

fn with_chat() -> TabLayout {
    let mut layout = blank();
    layout.open(A, |_| false);
    layout
}

#[test]
fn ids_tell_chats_from_documents() {
    assert_eq!(chat(Path::new("/s/a")), A);
    assert_eq!(chat_dir(A), Some(Path::new("/s/a")));
    assert_eq!(chat_dir("files:src/a.rs"), None);
    assert_eq!(split_document(A), None);
    assert_eq!(
        split_document("files:src/a:b.rs"),
        Some(("files", "src/a:b.rs"))
    );
}

#[test]
fn opening_avoids_covering_a_chat() {
    let mut layout = with_chat();
    layout.open("files", |_| false);
    assert_eq!(shape(&layout), "0[a, files*]");

    let mut layout = with_chat();
    layout.open("files", |_| true);
    assert_eq!(shape(&layout), "row(0[a*] | 1[files*])");
    assert_eq!(layout.focused, PaneId(1));

    // Opening an open tab only shows it; the chat pane stays as it is.
    layout.focus(PaneId(0));
    layout.open("notes", |_| true);
    layout.open("files", |_| true);
    assert_eq!(shape(&layout), "row(0[a*] | 1[files*, notes])");

    // Another chat joins the chat in view, even from the documents pane.
    layout.open(B, |_| true);
    assert_eq!(shape(&layout), "row(0[a, b*] | 1[files*, notes])");

    // With chats everywhere, a document covers the neighbor's chat.
    layout.move_tab(A, PaneId(1), None);
    layout.focus(PaneId(0));
    layout.open("todo", |_| true);
    assert_eq!(shape(&layout), "row(0[b*] | 1[files, notes, a, todo*])");
}

#[test]
fn closing_falls_back_and_drops_empty_panes() {
    let mut layout = with_chat();
    layout.open("files", |_| false);
    layout.open("notes", |_| false);
    layout.activate(PaneId(0), "files");
    layout.activate(PaneId(0), "notes");
    layout.close("notes");
    assert_eq!(shape(&layout), "0[a, files*]");

    layout.split_active(super::Edge::Right);
    assert_eq!(shape(&layout), "row(0[a*] | 1[files*])");
    layout.close("files");
    assert_eq!(shape(&layout), "0[a*]");
    assert_eq!(layout.focused, PaneId(0));

    // The last chat closes too, leaving an empty pane.
    layout.close(A);
    assert_eq!(shape(&layout), "0[]");
}

#[test]
fn retain_drops_gone_chats_and_documents() {
    let mut layout = with_chat();
    layout.open(B, |_| false);
    layout.open("files", |_| true);
    layout.retain(|id| id != B && id != "files");
    assert_eq!(shape(&layout), "0[a*]");
    layout.retain(|_| false);
    assert_eq!(shape(&layout), "0[]");
}

#[test]
fn active_chat_follows_focus_and_survives_hiding() {
    let mut layout = blank();
    assert_eq!(layout.active_chat(None), None);
    layout.open(A, |_| true);
    layout.open("files", |_| true);
    layout.open(B, |_| true);
    assert_eq!(shape(&layout), "row(0[a, b*] | 1[files*])");
    assert_eq!(layout.focused, PaneId(0));
    assert_eq!(layout.active_chat(Some(A)).as_deref(), Some(B));

    // A document in focus keeps the last chat; a closed one is replaced by
    // a chat in view.
    layout.focus(PaneId(1));
    assert_eq!(layout.active_chat(Some(A)).as_deref(), Some(A));
    layout.close(A);
    assert_eq!(layout.active_chat(Some(A)).as_deref(), Some(B));

    // Hidden behind a document, a chat still counts when nothing else does.
    layout.move_tab("files", PaneId(0), None);
    assert_eq!(shape(&layout), "0[b, files*]");
    assert_eq!(layout.chats().collect::<Vec<_>>(), [B]);
    assert_eq!(layout.visible().collect::<Vec<_>>(), ["files"]);
    assert_eq!(layout.active_chat(None).as_deref(), Some(B));
}

#[test]
fn preview_tab_is_replaced_until_kept() {
    let mut layout = with_chat();
    layout.open_preview("files:a.rs", |_| false);
    layout.open_preview("files:b.rs", |_| false);
    assert_eq!(shape(&layout), "0[a, files:b.rs*]");
    assert_eq!(layout.preview.as_deref(), Some("files:b.rs"));

    // A kept tab stays; the next preview opens next to it.
    layout.keep("files:b.rs");
    layout.open_preview("files:c.rs", |_| false);
    layout.open_preview("files:b.rs", |_| false);
    assert_eq!(shape(&layout), "0[a, files:b.rs*, files:c.rs]");
    assert_eq!(layout.preview.as_deref(), Some("files:c.rs"));

    // Closing the preview tab forgets it.
    layout.close("files:c.rs");
    assert_eq!(layout.preview, None);

    // Chats preview the same way; moving a preview keeps it.
    layout.open_preview(B, |_| false);
    layout.open_preview("chat:/s/c", |_| false);
    assert_eq!(shape(&layout), "0[a, files:b.rs, c*]");
    layout.split("chat:/s/c", PaneId(0), super::Edge::Bottom);
    assert_eq!(layout.preview, None);
    assert_eq!(shape(&layout), "col(0[a, files:b.rs*] / 1[c*])");
}
