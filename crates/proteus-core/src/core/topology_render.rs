mod helpers;
mod map;
mod markdown;
mod mermaid;
mod runtime;
#[cfg(test)]
mod slot_order_tests;
mod table;

pub use map::render_topology_map;
pub use markdown::render_topology_markdown;
pub use mermaid::render_topology_mermaid;
pub use runtime::{render_topology_runtime_mermaid, render_topology_runtime_path};
pub use table::render_topology_table;

#[test]
fn map_reports_how_many_dangling_nodes_were_omitted() {
    use crate::core::{ToolTopology, TopologySnapshot};

    let mut snapshot = TopologySnapshot::default();
    for i in 0..24 {
        snapshot.tools.push(ToolTopology {
            name: i.to_string(),
            ..Default::default()
        });
    }
    // 24 инструмента плюс «config» и «tools»: за пределом списка остались два узла.
    assert!(render_topology_map(&snapshot).contains("\n  ... and 2 more\n"));
}
