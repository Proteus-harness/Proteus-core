//! Adding, editing and removing profile MCP servers and package folders.

use std::path::{Path, PathBuf};

use proteus_contracts::app_protocol::config_builder::ConfigBuilderSnapshot;
use proteus_contracts::domain::{ToolSafety, mcp_protocol_version};
use serde_json::json;

use super::addons_snapshot;
use crate::settings::Draft;
use crate::settings::draft::Area;
use crate::settings::history::describe_changes;
use crate::settings::mcp::{McpProblem, McpText, mcp_problems, mcp_text};

/// Profile with the server `db` switched off.
fn baseline() -> Draft {
    let mut draft = Draft::default();
    draft.addons.mcp_servers =
        serde_json::from_value(json!([{ "name": "db", "command": "db-mcp" }])).expect("servers");
    draft.addons.addons.disabled_mcp_servers = vec!["db".into()];
    draft
}

#[test]
fn renamed_server_keeps_its_switch_and_saved_entry() {
    let baseline = baseline();
    let mut draft = baseline.clone();
    draft.set_mcp_text(0, McpText::Name, " base ").unwrap();
    assert_eq!(draft.addons.mcp_servers[0].name, "base");
    assert_eq!(draft.addons.addons.disabled_mcp_servers, ["base"]);
    assert!(!draft.mcp_enabled("base"));
    assert_eq!(
        draft.mcp_baseline(&baseline, 0),
        baseline.addons.mcp_servers.first()
    );
    draft.set_mcp_text(0, McpText::Name, "db").unwrap();
    assert!(!draft.is_dirty(&baseline));
}

#[test]
fn new_server_is_checked_before_saving_and_parses_its_fields() {
    let addons = addons_snapshot();
    let baseline = baseline();
    let mut draft = baseline.clone();
    let ix = draft.add_mcp();
    let server = &draft.addons.mcp_servers[ix];
    assert_eq!(
        (ix, server.name.as_str(), server.command.as_str()),
        (1, "server", "")
    );
    assert_eq!(server.safety, ToolSafety::RunsCommands);
    assert_eq!(server.protocol_version, mcp_protocol_version());
    assert_eq!(draft.mcp_baseline(&baseline, ix), None);
    assert_eq!(draft.add_mcp(), 2);
    assert_eq!(draft.addons.mcp_servers[2].name, "server-2");
    draft.remove_mcp(2);

    let problem = |index, field, message| McpProblem {
        index,
        field,
        message,
    };
    let no_command = problem(ix, McpText::Command, "Укажите команду");
    assert_eq!(mcp_problems(&draft, Some(&addons)), [no_command.clone()]);
    // A package server and another profile server hold these names.
    draft.set_mcp_text(ix, McpText::Name, "team:x").unwrap();
    assert_eq!(
        mcp_problems(&draft, Some(&addons)),
        [
            problem(ix, McpText::Name, "Имя уже занято"),
            no_command.clone()
        ]
    );
    draft.set_mcp_text(ix, McpText::Name, "db").unwrap();
    assert_eq!(
        mcp_problems(&draft, Some(&addons)),
        [
            problem(0, McpText::Name, "Имя уже занято"),
            problem(ix, McpText::Name, "Имя уже занято"),
            no_command.clone(),
        ]
    );
    draft.set_mcp_text(ix, McpText::Name, "  ").unwrap();
    assert_eq!(
        mcp_problems(&draft, Some(&addons))[0],
        problem(ix, McpText::Name, "Укажите имя")
    );
    // The duplicate's rename left the switch of `db` alone.
    assert_eq!(draft.addons.addons.disabled_mcp_servers, ["db"]);

    for (field, text) in [
        (McpText::Name, "files"),
        (McpText::Command, " files-mcp "),
        (McpText::Args, "--root\n\n /tmp \n"),
        (McpText::Cwd, " ~/work "),
        (McpText::Env, "A=1\nB = two\n"),
        (McpText::TimeoutMs, "500"),
        (McpText::ProtocolVersion, " "),
    ] {
        draft.set_mcp_text(ix, field, text).unwrap();
    }
    assert!(mcp_problems(&draft, Some(&addons)).is_empty());
    assert_eq!(
        draft.set_mcp_text(ix, McpText::Env, "A=1\noops"),
        Err("Строка 2: нужно KEY=VALUE".to_string())
    );
    assert_eq!(
        draft.set_mcp_text(ix, McpText::Env, "A=1\nA=2"),
        Err("A задана дважды".to_string())
    );
    assert!(draft.set_mcp_text(ix, McpText::TimeoutMs, "0").is_err());
    let server = &draft.addons.mcp_servers[ix];
    assert_eq!(server.command, "files-mcp");
    assert_eq!(server.args, ["--root", "/tmp"]);
    assert_eq!(server.cwd.as_deref(), Some(Path::new("~/work")));
    assert_eq!(mcp_text(server, McpText::Env), "A=1\nB=two");
    assert_eq!(server.timeout_ms, Some(500));
    assert_eq!(server.protocol_version, mcp_protocol_version());

    assert_eq!(draft.areas(&baseline), [Area::Mcp]);
    assert_eq!(
        describe_changes(&ConfigBuilderSnapshot::default(), &baseline, &draft),
        ["MCP: добавлен files"]
    );
    draft.remove_mcp(0);
    assert!(draft.addons.addons.disabled_mcp_servers.is_empty());
    assert_eq!(draft.addons.mcp_servers.len(), 1);
}

#[test]
fn package_folders_are_listed_once_and_removed() {
    let mut baseline = Draft::default();
    baseline.addons.addons.plugins =
        serde_json::from_value(json!([{ "path": "pkg", "enabled": false }])).expect("plugins");
    let mut draft = baseline.clone();
    draft.add_package(PathBuf::from("/work/new"));
    draft.add_package(PathBuf::from("/work/new"));
    draft.add_package(PathBuf::from("pkg"));
    let listed: Vec<_> = draft
        .addons
        .addons
        .plugins
        .iter()
        .map(|plugin| (plugin.path.to_string_lossy().to_string(), plugin.enabled))
        .collect();
    assert_eq!(
        listed,
        [("pkg".to_string(), true), ("/work/new".to_string(), true)]
    );
    assert_eq!(draft.areas(&baseline), [Area::Packages]);
    draft.remove_package(Path::new("/work/new"));
    draft.set_package(Path::new("pkg"), false);
    assert!(!draft.is_dirty(&baseline));
}
