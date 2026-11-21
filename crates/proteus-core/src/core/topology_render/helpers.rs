use crate::core::{SlotTopology, TopologySnapshot};

pub(super) fn ordered_slots(snapshot: &TopologySnapshot) -> Vec<&SlotTopology> {
    let mut slots = snapshot.slots.iter().collect::<Vec<_>>();
    slots.sort_by_key(|slot| slot.order);
    slots
}

pub(super) fn active_module_source(
    snapshot: &TopologySnapshot,
    slot_id: &str,
    module_id: &str,
) -> &'static str {
    snapshot
        .modules
        .iter()
        .find(|module| module.slot == slot_id && module.id == module_id)
        .map(|module| module.source.kind())
        .unwrap_or("unknown")
}

pub(super) fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(super) fn mermaid_label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn plain_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
