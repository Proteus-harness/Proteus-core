//! Translate client input into the existing canonical text/config surfaces.
use super::invalid;
use crate::{core::AppConfig, domain::PermissionMode};
use agent_client_protocol::{Result, schema::v1::*};

pub(super) fn prompt_text(mut blocks: Vec<ContentBlock>) -> Result<String> {
    // Keep an oversized source buffer out of the admitted prompt.
    let text = if let [ContentBlock::Text(text)] = blocks.as_mut_slice()
        && text.text.capacity() == text.text.len()
    {
        std::mem::take(&mut text.text)
    } else {
        let mut parts = Vec::with_capacity(blocks.len());
        for block in blocks {
            parts.push(match block {
                ContentBlock::Text(text) => text.text,
                ContentBlock::ResourceLink(link) => format!("{} ({})", link.name, link.uri),
                _ => {
                    return Err(invalid(
                        "unsupported prompt content; use text or resource_link",
                    ));
                }
            });
        }
        parts.join("\n")
    };
    if text.trim().is_empty() {
        return Err(invalid("prompt must not be empty"));
    }
    Ok(text)
}

pub(super) fn session_config(mut config: AppConfig, servers: Vec<McpServer>) -> Result<AppConfig> {
    for server in servers {
        let McpServer::Stdio(server) = server else {
            return Err(invalid("only stdio MCP servers are supported"));
        };
        if server.name.trim().is_empty()
            || config
                .tools
                .mcp_servers
                .iter()
                .any(|s| s.name == server.name)
        {
            return Err(invalid(
                "MCP server names must be nonempty and unique within the session",
            ));
        }
        let mut env = std::collections::BTreeMap::new();
        for entry in server.env {
            if entry.name.is_empty() || env.insert(entry.name, entry.value).is_some() {
                return Err(invalid("MCP environment names must be nonempty and unique"));
            }
        }
        // Deserialize through the current config schema so its MCP defaults and
        // ordinary ToolRegistry/policy path apply equally to editor-provided servers.
        let configured = serde_json::from_value(serde_json::json!({
            "name": server.name, "command": server.command, "args": server.args,
            "env": env,
        }))
        .map_err(super::internal)?;
        config.tools.mcp_servers.push(configured);
    }
    Ok(config)
}

pub(super) fn permission_mode(id: &SessionModeId) -> Result<PermissionMode> {
    match id.0.as_ref() {
        "normal" => Ok(PermissionMode::Normal),
        "plan" => Ok(PermissionMode::Plan),
        "auto" => Ok(PermissionMode::Auto),
        "full_access" => Ok(PermissionMode::FullAccess),
        _ => Err(invalid("unknown modeId")),
    }
}

pub(super) fn modes(mode: PermissionMode) -> Result<SessionModeState> {
    let mut available: Vec<_> = PermissionMode::SELECTABLE.into_iter().map(|mode| {
        let (name, description) = match mode {
            PermissionMode::Plan => ("Только чтение", "Доступны только инструменты, объявленные как чтение"),
            PermissionMode::Normal => ("С подтверждением", "Подтверждения и запреты определяются правилами выбранного профиля"),
            PermissionMode::FullAccess => ("Полный доступ", "Вызовы не требуют подтверждения; явные запреты правил и ограничения среды сохраняются"),
            PermissionMode::Auto => unreachable!("legacy mode is not selectable"),
        };
        SessionMode::new(mode.as_str(), name).description(description)
    }).collect();
    if mode == PermissionMode::Auto {
        available.push(
            SessionMode::new("auto", "Автоматически (прежний режим)").description(
                "Разрешены чтение и запись; команды, сеть и опасные инструменты запрещены",
            ),
        );
    }
    Ok(SessionModeState::new(mode.as_str(), available))
}
