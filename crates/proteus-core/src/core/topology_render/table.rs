use std::fmt::Write as _;

use crate::core::TopologySnapshot;

use super::helpers::{module_source_label, ordered_slots, yes_no};

pub fn render_topology_table(snapshot: &TopologySnapshot) -> String {
    let mut lines = Vec::new();
    lines.push("Proteus topology".to_owned());
    lines.push(format!("profile: {}", snapshot.profile));
    lines.push(format!("cwd: {}", snapshot.cwd));
    lines.push(format!("module epoch: {}", snapshot.module_epoch));
    if let Some(model) = &snapshot.model {
        lines.push(format!("model: {}/{}", model.provider, model.name));
    }
    lines.push(String::new());
    lines.push(format!("Hooks (ordered): {}", snapshot.hooks.join(" -> ")));
    lines.push("Active slots:".to_owned());
    lines.push(render_table(
        ["slot", "active_module", "source"],
        &ordered_slots(snapshot)
            .into_iter()
            .filter_map(|slot| {
                let active = slot.active_module.as_ref()?;
                let source = snapshot
                    .modules
                    .iter()
                    .find(|module| module.slot == slot.id && module.id == *active)
                    .map(|module| module_source_label(&module.source))
                    .unwrap_or_else(|| "-".to_owned());
                Some([slot.id.clone(), active.clone(), source])
            })
            .collect::<Vec<_>>(),
    ));
    lines.push(String::new());
    lines.push("Tools:".to_owned());
    lines.push(render_table(
        ["tool", "safety", "source", "enabled", "registered"],
        &snapshot
            .tools
            .iter()
            .map(|tool| {
                [
                    tool.name.clone(),
                    tool.safety.clone(),
                    tool.source.clone(),
                    yes_no(tool.enabled).to_owned(),
                    yes_no(tool.registered).to_owned(),
                ]
            })
            .collect::<Vec<_>>(),
    ));
    if !snapshot.warnings.is_empty() {
        lines.push(String::new());
        lines.push("Warnings:".to_owned());
        for warning in &snapshot.warnings {
            lines.push(format!("- {}: {}", warning.severity, warning.message));
        }
    }
    lines.join("\n")
}

fn render_table<const N: usize>(headers: [&str; N], rows: &[[String; N]]) -> String {
    let mut widths = headers.map(|header| header.chars().count());
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }

    let mut rendered = String::new();
    push_table_row(&mut rendered, &headers, &widths);
    rendered.push('\n');
    for (index, &width) in widths.iter().enumerate() {
        if index != 0 {
            rendered.push_str("  ");
        }
        rendered.extend(std::iter::repeat_n('-', width));
    }
    for row in rows {
        rendered.push('\n');
        push_table_row(&mut rendered, row, &widths);
    }
    rendered
}

fn push_table_row<const N: usize>(
    out: &mut String,
    row: &[impl AsRef<str>; N],
    widths: &[usize; N],
) {
    for (index, (cell, &width)) in row.iter().zip(widths).enumerate() {
        if index != 0 {
            out.push_str("  ");
        }
        let cell = cell.as_ref();
        write!(out, "{cell:width$}").expect("writing to a String cannot fail");
    }
}
