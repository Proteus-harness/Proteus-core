use super::super::{Edge, Node, PaneId, Size, TabLayout, drop_edge};
use super::shape;

/// One pane holding the given tabs, the first one shown.
fn single(tabs: &[&str]) -> TabLayout {
    let mut layout = super::blank();
    for tab in tabs {
        layout.open(tab, |_| false);
    }
    layout.activate(PaneId(0), tabs[0]);
    layout
}

/// `row(col(0[a*] / 2[c*]) | col(3[d*] / 1[b*]))`.
fn grid() -> TabLayout {
    let mut layout = single(&["a", "b", "c", "d"]);
    layout.split("b", PaneId(0), Edge::Right);
    layout.split("c", PaneId(0), Edge::Bottom);
    layout.split("d", PaneId(1), Edge::Top);
    layout
}

pub(super) fn weights(layout: &TabLayout, path: &[usize]) -> Vec<f32> {
    let sizes = layout.sizes(path).expect("split exists");
    sizes
        .into_iter()
        .map(|size| match size {
            Size::Weight(weight) => weight,
            Size::Fixed(px) => -px,
        })
        .collect()
}

#[test]
fn splits_by_four_edges() {
    let mut layout = grid();
    assert_eq!(
        shape(&layout),
        "row(col(0[a*] / 2[c*]) | col(3[d*] / 1[b*]))"
    );
    assert_eq!(layout.focused, PaneId(3));

    layout.open("e", |_| false);
    layout.split("e", PaneId(0), Edge::Left);
    assert_eq!(
        shape(&layout),
        "row(col(row(4[e*] | 0[a*]) / 2[c*]) | col(3[d*] / 1[b*]))"
    );
    assert_eq!(layout.focused, PaneId(4));

    // A pane cannot be split off its only tab.
    let before = layout.clone();
    layout.split("e", PaneId(4), Edge::Right);
    layout.split_active(Edge::Bottom);
    assert_eq!(layout, before);
}

#[test]
fn splitting_along_the_parent_axis_joins_the_parent() {
    let mut layout = single(&["a", "b", "c"]);
    layout.split("b", PaneId(0), Edge::Right);
    layout.split("c", PaneId(1), Edge::Right);
    assert_eq!(shape(&layout), "row(0[a*] | 1[b*] | 2[c*])");
    assert_eq!(weights(&layout, &[]), [1.5, 0.75, 0.75]);
    layout.reset(&[]);
    assert_eq!(weights(&layout, &[]), [1., 1., 1.]);
}

#[test]
fn normalization_collapses_and_merges_splits() {
    // row(0 | col(1 / row(2 | 3))): closing pane 1 lifts the inner row
    // into the outer one.
    let mut layout = single(&["a", "b", "c", "d"]);
    layout.split("b", PaneId(0), Edge::Right);
    layout.split("c", PaneId(1), Edge::Bottom);
    layout.split("d", PaneId(2), Edge::Right);
    assert_eq!(
        shape(&layout),
        "row(0[a*] | col(1[b*] / row(2[c*] | 3[d*])))"
    );
    layout.close("b");
    assert_eq!(shape(&layout), "row(0[a*] | 2[c*] | 3[d*])");
    assert_eq!(weights(&layout, &[]), [1.5, 0.75, 0.75]);

    // Emptying panes collapses the row down to one pane.
    layout.close("c");
    layout.close("a");
    assert_eq!(shape(&layout), "3[d*]");
    assert_eq!(layout.focused, PaneId(3));
}

#[test]
fn moving_tabs_between_panes() {
    let mut layout = grid();
    layout.open("e", |_| false);
    assert_eq!(
        shape(&layout),
        "row(col(0[a*] / 2[c*]) | col(3[d, e*] / 1[b*]))"
    );

    layout.move_tab("e", PaneId(0), Some("a"));
    assert_eq!(layout.focused, PaneId(0));
    layout.move_tab("a", PaneId(0), None);
    assert_eq!(
        shape(&layout),
        "row(col(0[e, a*] / 2[c*]) | col(3[d*] / 1[b*]))"
    );

    // Moving the last tab of a pane away closes the pane.
    layout.move_tab("d", PaneId(1), Some("b"));
    assert_eq!(shape(&layout), "row(col(0[e, a*] / 2[c*]) | 1[d*, b])");
    assert_eq!(layout.focused, PaneId(1));
}

#[test]
fn closing_the_last_tab_of_a_pane_focuses_a_neighbor() {
    let mut layout = grid();
    layout.focus(PaneId(2));
    layout.close("c");
    assert_eq!(shape(&layout), "row(0[a*] | col(3[d*] / 1[b*]))");
    assert_eq!(layout.focused, PaneId(0));

    layout.focus(PaneId(3));
    layout.close_pane(PaneId(3));
    assert_eq!(shape(&layout), "row(0[a, d*] | 1[b*])");
    assert_eq!(layout.focused, PaneId(0));
}

#[test]
fn focus_moves_by_geometry() {
    let mut layout = grid();
    layout.focus(PaneId(2));
    assert_eq!(layout.focus_neighbor(Edge::Right).as_deref(), Some("b"));
    assert_eq!(layout.focus_neighbor(Edge::Top).as_deref(), Some("d"));
    assert_eq!(layout.focus_neighbor(Edge::Top), None);
    assert_eq!(layout.focused, PaneId(3));
    assert_eq!(layout.focus_neighbor(Edge::Left).as_deref(), Some("a"));
    assert_eq!(layout.neighbor(PaneId(0), Edge::Bottom), Some(PaneId(2)));
}

#[test]
fn resizing_keeps_minimum_sizes() {
    let mut layout = grid();
    layout.resize(&[], 0, 0.3, 1., 0.1);
    assert_close(&weights(&layout, &[]), &[0.6, 1.4]);
    layout.resize(&[], 0, 0.01, 1., 0.1);
    assert_close(&weights(&layout, &[]), &[0.2, 1.8]);
    layout.resize(&[0], 0, 0.99, 1., 0.25);
    assert_close(&weights(&layout, &[0]), &[1.5, 0.5]);

    // A side with two panes along the axis needs twice the room.
    let mut layout = single(&["a", "b", "c", "d"]);
    layout.split("b", PaneId(0), Edge::Right);
    layout.split("c", PaneId(0), Edge::Bottom);
    layout.split("d", PaneId(0), Edge::Right);
    assert_eq!(
        shape(&layout),
        "row(col(row(0[a*] | 3[d*]) / 2[c*]) | 1[b*])"
    );
    layout.resize(&[], 0, 0., 1., 0.1);
    assert_close(&weights(&layout, &[]), &[0.4, 1.6]);
    layout.resize(&[], 0, 1., 1., 0.1);
    assert_close(&weights(&layout, &[]), &[1.8, 0.2]);

    // Weights that do not fit the split are ignored.
    let w = Size::Weight;
    layout.set_sizes(&[], &[w(1.), w(1.), w(1.)]);
    layout.set_sizes(&[], &[w(1.), w(0.)]);
    layout.set_sizes(&[], &[w(1.), Size::Fixed(300.)]);
    assert_close(&weights(&layout, &[]), &[1.8, 0.2]);
    layout.set_sizes(&[], &[w(1.), w(3.)]);
    assert_close(&weights(&layout, &[]), &[1., 3.]);
}

pub(super) fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?}");
    for (a, b) in actual.iter().zip(expected) {
        assert!((a - b).abs() < 1e-4, "{actual:?} != {expected:?}");
    }
}

#[test]
fn drop_zones_split_near_edges() {
    assert_eq!(drop_edge(0.5, 0.5), None);
    assert_eq!(drop_edge(0.1, 0.5), Some(Edge::Left));
    assert_eq!(drop_edge(0.9, 0.2), Some(Edge::Right));
    assert_eq!(drop_edge(0.5, 0.05), Some(Edge::Top));
    assert_eq!(drop_edge(0.4, 0.8), Some(Edge::Bottom));
}

#[test]
fn validation_rejects_broken_layouts() {
    let broken = |change: fn(&mut TabLayout)| {
        let mut layout = grid();
        change(&mut layout);
        layout.validate().is_err()
    };
    fn pane(layout: &mut TabLayout, id: u32) -> &mut super::super::Pane {
        layout.root.pane_mut(PaneId(id)).unwrap()
    }
    assert!(broken(|layout| pane(layout, 0).tabs.push("b".into())));
    assert!(broken(|layout| pane(layout, 0).active = Some("b".into())));
    assert!(broken(|layout| pane(layout, 0).active = None));
    assert!(broken(|layout| pane(layout, 0).id = PaneId(1)));
    assert!(broken(|layout| pane(layout, 0).id = PaneId(9)));
    assert!(broken(|layout| {
        let pane = pane(layout, 0);
        pane.tabs.clear();
        pane.active = None;
    }));
    assert!(broken(|layout| layout.focused = PaneId(7)));
    assert!(broken(|layout| layout.preview = Some("x".into())));
    assert!(broken(|layout| {
        let Node::Split(split) = &mut layout.root else {
            unreachable!()
        };
        split.children[0].size = Size::Weight(0.);
    }));
    assert!(broken(|layout| {
        let Node::Split(split) = &mut layout.root else {
            unreachable!()
        };
        split.children.pop();
    }));
    assert!(broken(|layout| {
        let Node::Split(split) = &mut layout.root else {
            unreachable!()
        };
        let mut inner = split.clone();
        inner.children[0].node = Node::Split(split.clone());
        layout.root = Node::Split(inner);
    }));
}

#[test]
fn layouts_round_trip_and_reject_other_shapes() {
    let mut layout = grid();
    layout.resize(&[], 0, 0.3, 1., 0.1);
    layout.open_preview("files:x", |_| false);
    let text = serde_json::to_string(&layout).unwrap();
    let back: TabLayout = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
    assert_eq!(shape(&back), shape(&layout));

    let old = r#"{"groups":[{"tabs":[],"active":null}],"focused":0,"split":500}"#;
    assert!(serde_json::from_str::<TabLayout>(old).is_err());
    let extra = text.replacen("\"focused\"", "\"zoom\":1,\"focused\"", 1);
    assert!(serde_json::from_str::<TabLayout>(&extra).is_err());
}
