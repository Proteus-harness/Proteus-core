use crate::core::TopologySnapshot;

use super::{
    helpers::{ordered_slots, yes_no},
    map::render_topology_map,
    runtime::render_topology_runtime_path,
};

pub fn render_topology_markdown(snapshot: &TopologySnapshot) -> String {
    let mut out = String::new();
    out.push_str("# Proteus Topology\n\n");
    out.push_str(&format!("profile: {}\n", md_code_span(&snapshot.profile)));
    out.push_str(&format!("cwd: {}\n", md_code_span(&snapshot.cwd)));
    out.push_str(&format!("module_epoch: `{}`\n", snapshot.module_epoch));
    out.push_str(&format!(
        "permission_mode: {}\n",
        md_code_span(&snapshot.permission_mode)
    ));
    if let Some(model) = &snapshot.model {
        out.push_str(&format!(
            "model: {}\n",
            md_code_span(&format!("{}/{}", model.provider, model.name))
        ));
    }
    if let Some(path) = &snapshot.config_path {
        out.push_str(&format!("config_path: {}\n", md_code_span(path)));
    }

    out.push_str(&format!(
        "\nhooks (ordered): {}\n",
        md_code_span(&snapshot.hooks.join(" -> "))
    ));
    out.push_str("\n## Runtime Path\n\n");
    out.push_str("```text\n");
    out.push_str(&render_topology_runtime_path(snapshot));
    out.push_str("\n```\n");

    out.push_str("\n## Diagnostic Map\n\n");
    out.push_str("```text\n");
    out.push_str(&render_topology_map(snapshot));
    out.push_str("\n```\n");

    out.push_str("\n## Active Slots\n\n");
    out.push_str("| Slot | Active Module | Source | Responsibility |\n");
    out.push_str("|---|---|---|---|\n");
    for slot in ordered_slots(snapshot) {
        let active = slot.active_module.as_deref().unwrap_or("-");
        let source = slot
            .active_module
            .as_ref()
            .and_then(|active| {
                snapshot
                    .modules
                    .iter()
                    .find(|module| module.slot == slot.id && module.id == *active)
            })
            .map(|module| module.source.kind())
            .unwrap_or("-");
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            md_cell(&slot.id),
            md_cell(active),
            md_cell(source),
            md_cell(&slot.responsibility)
        ));
    }

    out.push_str("\n## Tools\n\n");
    out.push_str("| Tool | Safety | Source | Enabled | Registered | Description |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for tool in &snapshot.tools {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            md_cell(&tool.name),
            md_cell(&tool.safety),
            md_cell(&tool.source),
            yes_no(tool.enabled),
            yes_no(tool.registered),
            md_cell(&tool.description)
        ));
    }

    out.push_str("\n## Warnings\n\n");
    if snapshot.warnings.is_empty() {
        out.push_str("(none)\n");
    } else {
        for warning in &snapshot.warnings {
            out.push_str(&format!(
                "- {} {}\n",
                md_code_span(&warning.severity),
                md_cell(&warning.message)
            ));
        }
    }

    out
}

fn md_cell(value: &str) -> String {
    let value = value.trim();
    let mut escaped = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                chars.next_if_eq(&'\n');
                escaped.push(' ');
            }
            '\n' => escaped.push(' '),
            '|' => escaped.push_str("\\|"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn md_code_span(value: &str) -> String {
    // Code spans strip one space at each end, except for all-space content.
    let padded_text =
        value.starts_with(' ') && value.ends_with(' ') && !value.trim_matches(' ').is_empty();
    if !value.contains('`') && !padded_text {
        return format!("`{value}`");
    }
    let longest_run = value.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    format!("{fence} {value} {fence}")
}

#[cfg(test)]
mod tests {
    use super::md_code_span;

    #[test]
    fn code_spans_preserve_literal_ticks_and_boundary_spaces() {
        for (value, markdown) in [
            ("/home/me/my work", "`/home/me/my work`"),
            ("  ", "`  `"),
            (" work ", "`  work  `"),
            ("/tmp/a`b", "`` /tmp/a`b ``"),
            ("`a``b`", "``` `a``b` ```"),
        ] {
            assert_eq!(md_code_span(value), markdown);
        }
    }
}

#[cfg(test)]
mod markdown_cell_tests {
    use super::md_cell;

    #[test]
    fn one_cell_flattens_each_logical_line_break_once() {
        for br in ["\n", "\r\n", "\r"] {
            assert_eq!(
                md_cell(&format!(" first|part{br}{br}last ")),
                "first\\|part  last"
            );
        }
    }
}
