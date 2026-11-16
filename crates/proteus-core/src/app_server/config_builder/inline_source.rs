//! Edits an inline role in its original document, preserving parent/sibling source.
use super::*;

fn selected_item_mut<'a>(
    document: &'a mut toml_edit::DocumentMut,
    selection: &[String],
) -> Result<&'a mut toml_edit::Item> {
    let mut current = document.as_item_mut();
    for name in selection {
        let roles = current
            .get_mut("agent_control")
            .and_then(|control| control.get_mut("roles"))
            .context("inline agent source must contain agent_control.roles")?;
        let count = roles
            .as_array_of_tables()
            .map(|roles| roles.len())
            .or_else(|| roles.as_array().map(|roles| roles.len()))
            .context("agent_control.roles must be an array")?;
        let mut matching = (0..count).filter(|&index| {
            roles
                .get(index)
                .and_then(|role| role.get("name"))
                .and_then(toml_edit::Item::as_str)
                == Some(name.as_str())
        });
        let index = matching
            .next()
            .with_context(|| format!("inline agent role is unavailable: {name}"))?;
        anyhow::ensure!(
            matching.next().is_none(),
            "duplicate inline agent role: {name}"
        );
        current = roles
            .get_mut(index)
            .and_then(|role| role.get_mut("config"))
            .context("inline agent role must contain a config object")?;
        anyhow::ensure!(
            current.is_table_like(),
            "inline agent role must contain a config object"
        );
    }
    Ok(current)
}

/// A deliberate child model edit is stored in the child config. Remove a
/// role-level default only for that edit, so the source never has two choices.
pub(super) fn remove_selected_role_model(
    document: &mut toml_edit::DocumentMut,
    selection: &[String],
) -> Result<()> {
    let Some((name, parents)) = selection.split_last() else {
        return Ok(());
    };
    let parent = selected_item_mut(document, parents)?;
    let roles = parent
        .get_mut("agent_control")
        .and_then(|control| control.get_mut("roles"))
        .context("inline agent source must contain agent_control.roles")?;
    let count = roles
        .as_array_of_tables()
        .map(|roles| roles.len())
        .or_else(|| roles.as_array().map(|roles| roles.len()))
        .context("agent_control.roles must be an array")?;
    let index = (0..count)
        .find(|&index| {
            roles
                .get(index)
                .and_then(|role| role.get("name"))
                .and_then(toml_edit::Item::as_str)
                == Some(name.as_str())
        })
        .context("inline agent role is unavailable")?;
    roles
        .get_mut(index)
        .and_then(toml_edit::Item::as_table_like_mut)
        .context("inline agent role must be an object")?
        .remove("model");
    Ok(())
}

pub(super) fn selected_toml_document(
    document: &mut toml_edit::DocumentMut,
    selection: &[String],
) -> Result<toml_edit::DocumentMut> {
    let selected = selected_item_mut(document, selection)?
        .clone()
        .into_table()
        .map_err(|_| anyhow!("inline agent config must be a table"))?;
    Ok(selected.into())
}

pub(super) fn replace_selected_toml_document(
    document: &mut toml_edit::DocumentMut,
    selection: &[String],
    edited: toml_edit::DocumentMut,
) -> Result<()> {
    let target = selected_item_mut(document, selection)?;
    let inline = target.as_inline_table().is_some();
    let table = edited.into_table();
    *target = if inline {
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table.into_inline_table()))
    } else {
        toml_edit::Item::Table(table)
    };
    // Positions originate in different parsed documents. Keeping them can emit
    // a newly added table after the next [[roles]] entry, attaching settings to
    // the wrong sibling. Tree order keeps each array entry and its descendants
    // together; comments/decorations remain attached to their original nodes.
    order_table_positions(document.as_table_mut(), &mut 0);
    Ok(())
}

fn order_table_positions(table: &mut toml_edit::Table, next: &mut usize) {
    table.set_position(*next);
    *next += 1;
    for (_, item) in table.iter_mut() {
        match item {
            toml_edit::Item::Table(table) => order_table_positions(table, next),
            toml_edit::Item::ArrayOfTables(tables) => {
                for table in tables.iter_mut() {
                    order_table_positions(table, next);
                }
            }
            _ => {}
        }
    }
}

/// A loaded AppConfig stores stable references, not the original inline bodies.
/// Losing the source must not silently replace those bodies with runtime refs.
pub(super) fn ensure_bootstrap_has_no_inline_references(config: &AppConfig) -> Result<()> {
    for role in &config.agent_control.roles {
        anyhow::ensure!(
            crate::core::config_selection(Path::new(&role.config))?.is_empty(),
            "cannot recreate an inline-agent config after its source was removed"
        );
    }
    Ok(())
}
