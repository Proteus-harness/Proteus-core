use super::super::{Edge, PANEL_WIDTH, PaneId, TabLayout};
use super::{A, B, shape};

const SESSIONS: &str = "panel:sessions";
const FILES: &str = "panel:files";
const PANELS: &str = "264px 1[@sessions^*, @files^]";

#[test]
fn fresh_layout_has_panels_and_a_place_for_chats() {
    let mut layout = TabLayout::default();
    assert_eq!(shape(&layout), format!("row({PANELS} | 0[])"));
    assert_eq!(layout.focused, PaneId(0));
    assert_eq!(PANEL_WIDTH, 264.);

    // The first chat fills the empty pane, even with the panels focused.
    layout.focus(PaneId(1));
    layout.open(A, |_| true);
    assert_eq!(shape(&layout), format!("row({PANELS} | 0[a*])"));

    // Closing the last chat keeps an empty pane next to the panels.
    layout.close(A);
    assert_eq!(shape(&layout), format!("row({PANELS} | 0[])"));
    layout.open(B, |_| true);
    layout.retain(|id| id != B);
    assert_eq!(shape(&layout), format!("row({PANELS} | 0[])"));
}

#[test]
fn chats_and_documents_skip_panes_of_panels() {
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.focus(PaneId(1));
    layout.open("files:x", |_| false);
    assert_eq!(shape(&layout), format!("row({PANELS} | 0[a, files:x*])"));

    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.focus(PaneId(1));
    layout.open("files:x", |pane| pane == PaneId(0));
    assert_eq!(
        shape(&layout),
        format!("row({PANELS} | 0[a*] | 2[files:x*])")
    );
    layout.focus(PaneId(1));
    layout.open(B, |_| true);
    assert_eq!(
        shape(&layout),
        format!("row({PANELS} | 0[a, b*] | 2[files:x*])")
    );

    // A pane with a chat put among the panels takes tabs like any other.
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.move_tab(A, PaneId(1), None);
    assert_eq!(shape(&layout), "1[@sessions^, @files^, a*]");
    layout.open(B, |_| true);
    assert_eq!(shape(&layout), "1[@sessions^, @files^, a, b*]");
}

#[test]
fn a_chosen_pane_takes_the_tab_even_among_panels() {
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.open_in("terminal:1", PaneId(1), |_| true);
    assert_eq!(
        shape(&layout),
        "row(264px 1[@sessions^, @files^, terminal:1*] | 0[a*])"
    );
    // An open tab only comes forward where it is.
    layout.open_in(A, PaneId(1), |_| true);
    assert_eq!(layout.pane_of(A), Some(PaneId(0)));
}

#[test]
fn without_a_pane_for_chats_one_is_added_on_the_right() {
    let mut layout = TabLayout::default();
    layout.move_tab(FILES, PaneId(0), None);
    assert_eq!(shape(&layout), "row(264px 1[@sessions^*] | 0[@files^*])");
    layout.open(A, |_| true);
    assert_eq!(
        shape(&layout),
        "row(264px 1[@sessions^*] | 0[@files^*] | 2[a*])"
    );

    // A lone pane of panels becomes the fixed side of a new row.
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.move_tab(A, PaneId(1), None);
    layout.close(A);
    assert_eq!(shape(&layout), "1[@sessions^*, @files^]");
    layout.open(B, |_| true);
    assert_eq!(shape(&layout), format!("row({PANELS} | 2[b*])"));
}

#[test]
fn panels_reopen_pinned_among_panels() {
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.close(SESSIONS);
    layout.close(FILES);
    assert_eq!(shape(&layout), "0[a*]");

    layout.open_panel(FILES, None);
    assert_eq!(shape(&layout), "row(264px 2[@files^*] | 0[a*])");
    assert_eq!(layout.focused, PaneId(2));
    layout.focus(PaneId(0));
    layout.open(SESSIONS, |_| true);
    assert_eq!(shape(&layout), "row(264px 2[@files^, @sessions^*] | 0[a*])");

    // An open panel only comes forward; a pane can be chosen.
    layout.open_panel(FILES, Some(PaneId(0)));
    assert_eq!(layout.active(PaneId(2)), Some(FILES));
    layout.open_panel("panel:notes", Some(PaneId(0)));
    assert_eq!(
        shape(&layout),
        "row(264px 2[@files^*, @sessions^] | 0[@notes^*, a])"
    );

    // Panels are never previews.
    layout.open_preview("panel:other", |_| true);
    assert_eq!(layout.preview, None);
    assert!(layout.is_pinned("panel:other"));
}

#[test]
fn closing_a_pane_prefers_one_of_the_same_kind() {
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.open(B, |_| true);
    layout.split(B, PaneId(0), Edge::Right);
    assert_eq!(shape(&layout), format!("row({PANELS} | 0[a*] | 2[b*])"));
    layout.close_pane(PaneId(0));
    assert_eq!(shape(&layout), format!("row({PANELS} | 2[b*, a])"));

    // Closing the only pane for chats while it is empty changes nothing.
    layout.close(A);
    layout.close(B);
    let before = layout.clone();
    layout.close_pane(PaneId(2));
    assert_eq!(layout.root, before.root);
}

#[test]
fn validation_knows_panels() {
    let mut layout = TabLayout::default();
    assert_eq!(layout.validate(), Ok(()));
    layout.preview = Some(SESSIONS.into());
    assert!(layout.validate().is_err());

    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.split(FILES, PaneId(1), Edge::Bottom);
    let mut broken = layout.clone();
    broken.root.pane_mut(PaneId(1)).unwrap().tabs.clear();
    broken.root.pane_mut(PaneId(1)).unwrap().pinned = 0;
    broken.root.pane_mut(PaneId(1)).unwrap().active = None;
    assert!(broken.validate().is_err());

    let mut broken = layout.clone();
    let pane = broken.root.pane_mut(PaneId(2)).unwrap();
    pane.tabs = vec!["panel:".into()];
    pane.active = Some("panel:".into());
    assert!(broken.validate().is_err());
}
