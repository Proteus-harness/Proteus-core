use super::super::geometry::Rect;
use super::super::{Edge, Node, PaneId, Size, TabLayout};
use super::shape;
use super::tree::{assert_close, weights};

const A: &str = "chat:/s/a";
const B: &str = "chat:/s/b";

/// `row(264px 0[a*] | 1[b*])`: a chat locked at 264 px beside another.
fn with_fixed() -> TabLayout {
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.open(B, |_| true);
    layout.split(B, PaneId(0), Edge::Right);
    layout.resize(&[], 0, 264., 1264., 240.);
    layout.lock_pane(PaneId(0), true, 1264., 800., 0.);
    layout
}

/// `row(0[a*] | 1[b*])` with sizes `sizes`, none of them locked.
fn sized(sizes: [Size; 2]) -> TabLayout {
    let mut layout = TabLayout::default();
    layout.open(A, |_| true);
    layout.open(B, |_| true);
    layout.split(B, PaneId(0), Edge::Right);
    let Node::Split(split) = &mut layout.root else {
        unreachable!()
    };
    for (child, size) in split.children.iter_mut().zip(sizes) {
        child.size = size;
    }
    layout
}

fn widths(layout: &TabLayout, width: f32) -> Vec<f32> {
    let rects: Vec<Rect> = layout
        .rects_in(width, 800.)
        .into_iter()
        .map(|(_, rect)| rect)
        .collect();
    rects.iter().map(|rect| rect.w).collect()
}

#[test]
fn fixed_panes_keep_their_pixels() {
    let layout = with_fixed();
    assert_close(&widths(&layout, 1264.), &[264., 1000.]);
    assert_close(&widths(&layout, 2264.), &[264., 2000.]);
    // Too narrow a window shrinks the fixed pane last of all.
    assert_close(&widths(&layout, 200.), &[200., 0.]);
}

#[test]
fn spaced_places_leave_room_for_dividers() {
    let layout = with_fixed();
    let rects: Vec<Rect> = layout
        .rects_spaced(1269., 800., 5.)
        .into_iter()
        .map(|(_, rect)| rect)
        .collect();
    assert_close(&[rects[0].x, rects[0].w], &[0., 264.]);
    assert_close(&[rects[1].x, rects[1].w], &[269., 1000.]);
}

#[test]
fn dividers_resize_fixed_panes_in_pixels() {
    let mut layout = with_fixed();
    layout.resize(&[], 0, 300., 1264., 240.);
    assert_eq!(
        layout.sizes(&[]),
        Some(vec![Size::Fixed(300.), Size::Weight(1.)])
    );
    layout.resize(&[], 0, 100., 1264., 240.);
    assert_eq!(layout.sizes(&[]).unwrap()[0], Size::Fixed(240.));
    // Sharing evenly leaves the fixed pane as it is.
    layout.reset(&[]);
    assert_eq!(
        layout.sizes(&[]),
        Some(vec![Size::Fixed(240.), Size::Weight(1.)])
    );

    // Between weighted panes, a fixed one stays as it is.
    let mut layout = with_fixed();
    layout.open("files:x", |_| true);
    layout.split("files:x", PaneId(1), Edge::Right);
    assert_eq!(shape(&layout), "row(264px 0[a*] | 1[b*] | 2[files:x*])");
    layout.resize(&[], 1, 264. + 250., 1264., 240.);
    assert_eq!(layout.sizes(&[]).unwrap()[0], Size::Fixed(264.));
    assert_close(&weights(&layout, &[])[1..], &[0.5, 1.5]);
    assert_close(&widths(&layout, 1264.), &[264., 250., 750.]);
}

#[test]
fn splitting_beside_a_fixed_pane() {
    let mut layout = with_fixed();
    layout.open("files:x", |_| false);
    layout.split("files:x", PaneId(1), Edge::Left);
    assert_eq!(shape(&layout), "row(264px 0[a*] | 2[files:x*] | 1[b*])");
    assert_close(&weights(&layout, &[])[1..], &[1., 1.]);
}

#[test]
fn normalizing_keeps_splits_resizable() {
    // Without weighted children, fixed ones share by their pixels.
    let mut layout = sized([Size::Fixed(264.), Size::Weight(1.)]);
    layout.open("files:x", |_| true);
    layout.split("files:x", PaneId(1), Edge::Right);
    let Node::Split(split) = &mut layout.root else {
        unreachable!()
    };
    split.children[2].size = Size::Fixed(300.);
    assert_eq!(
        shape(&layout),
        "row(264px 0[a*] | 1[b*] | 300px 2[files:x*])"
    );
    layout.move_tab(B, PaneId(0), None);
    assert_close(&weights(&layout, &[]), &[264. / 282., 300. / 282.]);

    // A split merged into a fixed child of its parent turns fixed too.
    let mut layout = sized([Size::Weight(1.), Size::Weight(1.)]);
    let (c, d) = ("chat:/s/c", "chat:/s/d");
    layout.open(c, |_| false);
    layout.split(c, PaneId(0), Edge::Bottom);
    layout.open(d, |_| false);
    layout.split(d, PaneId(2), Edge::Right);
    let Node::Split(split) = &mut layout.root else {
        unreachable!()
    };
    split.children[0].size = Size::Fixed(264.);
    assert_eq!(
        shape(&layout),
        "row(264px col(0[a*] / row(2[c*] | 3[d*])) | 1[b*])"
    );
    layout.close(A);
    assert_eq!(shape(&layout), "row(132px 2[c*] | 132px 3[d*] | 1[b*])");
}

#[test]
fn validation_checks_sizes() {
    let broken = |size: [Size; 2]| {
        let mut layout = with_fixed();
        let Node::Split(split) = &mut layout.root else {
            unreachable!()
        };
        split.children[0].size = size[0];
        split.children[1].size = size[1];
        layout.validate().is_err()
    };
    assert!(!broken([Size::Fixed(10.), Size::Weight(0.5)]));
    assert!(broken([Size::Fixed(10.), Size::Fixed(20.)]));
    assert!(broken([Size::Fixed(0.), Size::Weight(1.)]));
    assert!(broken([Size::Fixed(f32::NAN), Size::Weight(1.)]));
    assert!(broken([Size::Fixed(10.), Size::Weight(f32::INFINITY)]));
}

#[test]
fn fresh_layouts_round_trip() {
    let layout = with_fixed();
    let text = serde_json::to_string(&layout).unwrap();
    assert!(text.contains(r#""size":{"fixed":264.0}"#), "{text}");
    let back: TabLayout = serde_json::from_str(&text).unwrap();
    assert_eq!(back, layout);

    // Weights of the earlier form are not read.
    let old = text.replace(r#""size":{"fixed":264.0}"#, r#""weight":1.0"#);
    assert!(serde_json::from_str::<TabLayout>(&old).is_err());
    let unpinned = text.replace(r#""pinned":0,"#, "");
    assert!(serde_json::from_str::<TabLayout>(&unpinned).is_err());
}

#[test]
fn a_closed_pane_widens_its_neighbour_only() {
    let mut layout = TabLayout::default();
    let (b, c) = (B, "chat:/s/c");
    layout.open(A, |_| false);
    layout.open(b, |_| false);
    layout.split(b, layout.pane_of(b).unwrap(), Edge::Right);
    layout.open(c, |_| false);
    layout.split(c, layout.pane_of(c).unwrap(), Edge::Right);
    assert_close(&widths(&layout, 1200.), &[600., 300., 300.]);

    // Moving the last tab of the third pane into the second one leaves
    // the first pane as wide as it was.
    layout.move_tab(c, layout.pane_of(b).unwrap(), None);
    assert_close(&widths(&layout, 1200.), &[600., 600.]);
}

#[test]
fn a_locked_pane_keeps_its_pixels_until_unlocked() {
    let mut layout = TabLayout::default();
    let b = B;
    layout.open(A, |_| false);
    layout.open(b, |_| false);
    layout.split(b, layout.pane_of(b).unwrap(), Edge::Right);
    let pane = layout.pane_of(b).unwrap();
    assert_eq!(layout.pane_locked(pane), Some(false));

    layout.lock_pane(pane, true, 1200., 800., 0.);
    assert_eq!(layout.pane_locked(pane), Some(true));
    // The other pane can take the lock over.
    assert_eq!(layout.pane_locked(layout.pane_of(A).unwrap()), Some(false));
    assert_close(&widths(&layout, 1600.), &[1000., 600.]);

    // Nothing splits a locked pane.
    let c = "chat:/s/c";
    layout.open(c, |_| false);
    layout.move_tab(c, pane, None);
    assert!(!layout.can_split(c, pane));
    layout.split(c, pane, Edge::Bottom);
    assert_eq!(layout.pane_count(), 2);

    // Closing the last tab beside it leaves an empty pane holding the room,
    // so the locked one keeps its pixels.
    layout.close(A);
    assert_eq!(layout.pane_of(A), None);
    assert_eq!(layout.pane_count(), 2);
    assert_eq!(layout.pane_locked(pane), Some(true));
    let room = layout.panes()[0].id;
    assert!(layout.holds_room(room));
    assert!(layout.validate().is_ok());
    assert_close(&widths(&layout, 1600.), &[1000., 600.]);

    // Closing that pane hands its room over; alone in the tree the lock
    // means nothing and is forgotten.
    assert!(layout.can_close(room));
    layout.close_pane(room);
    assert_eq!(layout.pane_count(), 1);
    assert_eq!(layout.pane_locked(pane), None);
    assert!(!layout.panes()[0].locked);
    assert!(layout.can_split(c, pane));
}

#[test]
fn moving_the_last_tab_into_a_locked_pane_leaves_room() {
    let mut layout = with_fixed();
    let (locked, other) = (PaneId(0), PaneId(1));
    layout.move_tab(B, locked, None);
    assert_eq!(layout.pane_count(), 2);
    assert!(layout.holds_room(other));
    assert!(layout.validate().is_ok());
    assert_close(&widths(&layout, 1600.), &[264., 1336.]);
    // The layout survives saving, empty pane and all.
    let text = serde_json::to_string(&layout).unwrap();
    let restored: TabLayout = serde_json::from_str(&text).unwrap();
    assert!(restored.validate().is_ok());

    // An empty pane takes the next chat; closing it brings the room back.
    let c = "chat:/s/c";
    layout.open(c, |_| true);
    assert_eq!(layout.pane_of(c), Some(other));
    layout.close(c);
    assert!(layout.holds_room(other));
    // Unlocked, the neighbour takes the room itself.
    layout.lock_pane(locked, false, 1264., 800., 0.);
    assert_eq!(layout.pane_count(), 1);
    assert_eq!(layout.pane_locked(locked), None);
}

#[test]
fn locking_beside_a_fixed_pane_hands_it_the_rest() {
    let mut layout = sized([Size::Fixed(264.), Size::Weight(1.)]);
    let (fixed, chat) = (PaneId(0), PaneId(1));
    assert_eq!(layout.pane_locked(chat), Some(false));
    layout.lock_pane(chat, true, 1200., 800., 0.);
    assert_close(&widths(&layout, 1600.), &[664., 936.]);

    // Locking the other one takes the lock over.
    layout.lock_pane(fixed, true, 1600., 800., 0.);
    assert_eq!(layout.pane_locked(chat), Some(false));
    assert_eq!(layout.pane_locked(fixed), Some(true));
    assert_close(&widths(&layout, 1200.), &[664., 536.]);
}

#[test]
fn unlocking_a_pane_lets_it_keep_its_share() {
    let mut layout = TabLayout::default();
    let b = B;
    layout.open(A, |_| false);
    layout.open(b, |_| false);
    layout.split(b, layout.pane_of(b).unwrap(), Edge::Right);
    let pane = layout.pane_of(b).unwrap();
    layout.lock_pane(pane, true, 1200., 800., 0.);
    // Unlocked, it keeps its current share as the window changes.
    layout.lock_pane(pane, false, 1600., 800., 0.);
    assert_close(&widths(&layout, 800.), &[500., 300.]);
}
