//! Session selectors project the same provider-owned selection as HTTP/stdio.
use agent_client_protocol::{Client, ConnectionTo, Result, schema::v1::*};

use super::{Session, internal, invalid};
use crate::app_server::AppServerHandle;
use proteus_contracts::app_protocol::settings::{
    SessionSetting, SessionSettingControl, SessionSettingUpdate,
};

pub(super) async fn options(server: &AppServerHandle) -> Result<Vec<SessionConfigOption>> {
    Ok(render(server.session_settings().await))
}

fn render(settings: Vec<SessionSetting>) -> Vec<SessionConfigOption> {
    settings
        .into_iter()
        .filter_map(|setting| {
            let SessionSettingControl::Select { value, options } = setting.control else {
                return None;
            };
            let mut option = SessionConfigOption::select(
                setting.id,
                setting.name,
                value,
                options
                    .into_iter()
                    .map(|choice| {
                        SessionConfigSelectOption::new(choice.value, choice.name)
                            .description(choice.description)
                    })
                    .collect::<Vec<_>>(),
            );
            option = match setting.category.as_deref() {
                Some("mode") => option.category(SessionConfigOptionCategory::Mode),
                Some("model") => option.category(SessionConfigOptionCategory::Model),
                Some("thought_level") => option.category(SessionConfigOptionCategory::ThoughtLevel),
                _ => option,
            };
            Some(option)
        })
        .collect()
}

pub(super) async fn set(
    session: Session,
    request: SetSessionConfigOptionRequest,
    cx: ConnectionTo<Client>,
) -> Result<SetSessionConfigOptionResponse> {
    let _lease = session.reserve()?;
    let value = request
        .value
        .as_value_id()
        .ok_or_else(|| invalid("configuration selectors require a string value"))?;
    session
        .server
        .set_session_setting(SessionSettingUpdate {
            id: request.config_id.0.to_string(),
            value: value.0.to_string().into(),
        })
        .await
        .map_err(|error| {
            if error.is::<crate::app_server::session_settings::InvalidSessionSetting>() {
                invalid(format!("{error:#}"))
            } else {
                internal(error)
            }
        })?;
    if request.config_id.0.as_ref() == "mode" {
        cx.send_notification(SessionNotification::new(
            request.session_id.clone(),
            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(value.0.to_string())),
        ))?;
    }
    let options = options(&session.server).await?;
    notify(&cx, request.session_id, options.clone())?;
    Ok(SetSessionConfigOptionResponse::new(options))
}

pub(super) fn notify(
    cx: &ConnectionTo<Client>,
    session: SessionId,
    options: Vec<SessionConfigOption>,
) -> Result<()> {
    cx.send_notification(SessionNotification::new(
        session,
        SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
    ))
}

#[cfg(test)]
mod tests;
