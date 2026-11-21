use super::{render_topology_markdown, render_topology_table};
use crate::core::{SlotTopology, TopologySnapshot};

#[test]
fn slot_tables_follow_display_order_without_changing_their_membership() {
    let slot = |id: &str, order, active: bool| SlotTopology {
        id: id.to_owned(),
        order,
        active_module: active.then(|| "example".to_owned()),
        ..Default::default()
    };
    let snapshot = TopologySnapshot {
        slots: vec![
            slot("custom", 900, true),
            slot("first", 1, false),
            slot("beta", 10, true),
            slot("alpha", 10, true),
        ],
        ..Default::default()
    };

    let table = render_topology_table(&snapshot);
    let table_rows = table
        .split_once("Active slots:\n")
        .unwrap()
        .1
        .lines()
        .skip(2)
        .take_while(|line| !line.is_empty())
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(table_rows, ["beta", "alpha", "custom"]);

    let markdown = render_topology_markdown(&snapshot);
    let markdown_rows = markdown
        .split_once("## Active Slots\n\n")
        .unwrap()
        .1
        .lines()
        .skip(2)
        .take_while(|line| line.starts_with("| "))
        .map(|line| line.split('|').nth(1).unwrap().trim())
        .collect::<Vec<_>>();
    assert_eq!(markdown_rows, ["first", "beta", "alpha", "custom"]);
}
