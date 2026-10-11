use super::super::{Edge, PaneId, TabLayout};
use super::{blank, shape};

/// One pane with `a`..`e`, `c` shown.
fn five() -> TabLayout {
    let mut layout = blank();
    for tab in ["a", "b", "c", "d", "e"] {
        layout.open(tab, |_| false);
    }
    layout.activate(PaneId(0), "c");
    layout
}

#[test]
fn pinned_tabs_stay_in_front() {
    let mut layout = five();
    layout.set_pinned("d", true);
    layout.set_pinned("b", true);
    assert_eq!(shape(&layout), "0[d^, b^, a, c*, e]");
    layout.set_pinned("d", false);
    assert_eq!(shape(&layout), "0[b^, d, a, c*, e]");
    assert!(layout.is_pinned("b") && !layout.is_pinned("d"));

    // Unpinned tabs land after the pinned ones, pinned ones among them.
    layout.move_tab("e", PaneId(0), Some("b"));
    assert_eq!(shape(&layout), "0[b^, e*, d, a, c]");
    layout.set_pinned("a", true);
    layout.move_tab("a", PaneId(0), Some("c"));
    assert_eq!(shape(&layout), "0[b^, a^*, e, d, c]");

    // Pinning a preview keeps it.
    layout.open_preview("p", |_| false);
    layout.set_pinned("p", true);
    assert_eq!(layout.preview, None);
    assert_eq!(shape(&layout), "0[b^, a^, p^*, e, d, c]");
}

#[test]
fn closing_others_and_to_the_right_spares_pinned_tabs() {
    let mut layout = five();
    layout.set_pinned("d", true);
    assert_eq!(layout.others("b"), ["a", "c", "e"]);
    assert_eq!(layout.right_of("a"), ["b", "c", "e"]);
    assert_eq!(layout.right_of("e"), Vec::<String>::new());
    layout.close_right("b");
    assert_eq!(shape(&layout), "0[d^, a, b*]");

    let mut layout = five();
    layout.set_pinned("a", true);
    layout.close_others("c");
    assert_eq!(shape(&layout), "0[a^, c*]");
    // From a pinned tab, the others are the unpinned ones.
    layout.close_others("a");
    assert_eq!(shape(&layout), "0[a^*]");
}

#[test]
fn pins_travel_with_their_tabs() {
    let mut layout = five();
    layout.set_pinned("a", true);
    layout.set_pinned("b", true);
    layout.split("b", PaneId(0), Edge::Right);
    assert_eq!(shape(&layout), "row(0[a^, c*, d, e] | 1[b^*])");
    layout.open("x", |_| false);
    layout.move_tab("a", PaneId(1), None);
    assert_eq!(shape(&layout), "row(0[c*, d, e] | 1[b^, a^*, x])");

    layout.close_pane(PaneId(1));
    assert_eq!(shape(&layout), "0[b^, a^*, c, d, e, x]");
}

#[test]
fn validation_checks_pins() {
    let mut layout = five();
    layout.root.pane_mut(PaneId(0)).unwrap().pinned = 6;
    assert!(layout.validate().is_err());

    let mut layout = five();
    layout.open_preview("p", |_| false);
    layout.root.pane_mut(PaneId(0)).unwrap().pinned = 6;
    assert!(layout.validate().is_err());
    layout.root.pane_mut(PaneId(0)).unwrap().pinned = 5;
    assert!(layout.validate().is_ok());
}
