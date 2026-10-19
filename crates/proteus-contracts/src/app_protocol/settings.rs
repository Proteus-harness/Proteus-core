//! Server-owned session settings. Clients route by id, never by an enum of settings.

use serde::{Deserialize, Deserializer, Serialize, de::Error};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSetting {
    pub id: String,
    pub name: String,
    /// Presentation hint only; an unfamiliar category does not change behavior.
    pub category: Option<String>,
    pub control: SessionSettingControl,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionSettingControl {
    Select {
        value: String,
        options: Vec<SessionSettingChoice>,
    },
    /// Readers omit controls they cannot render and forward their opaque data.
    #[serde(untagged)]
    Unsupported(serde_json::Value),
}

impl<'de> Deserialize<'de> for SessionSettingControl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        match raw.get("type").and_then(serde_json::Value::as_str) {
            Some("select") => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Select {
                    #[serde(rename = "type")]
                    _kind: String,
                    value: String,
                    options: Vec<SessionSettingChoice>,
                }
                let control: Select = serde_json::from_value(raw).map_err(D::Error::custom)?;
                Ok(Self::Select {
                    value: control.value,
                    options: control.options,
                })
            }
            Some(_) => Ok(Self::Unsupported(raw)),
            None => Err(D::Error::custom(
                "session setting control requires a string type",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSettingChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSettingUpdate {
    pub id: String,
    /// The setting's control defines the type; the transport stays generic.
    pub value: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn readers_skip_unknown_controls_but_keep_known_controls_strict() {
        let mut setting = json!({"id":"org.example/temperature", "name":"Temperature", "category":"future-category",
            "control":{"type":"future-slider", "min":0, "max":2, "value":1}});
        let decoded: SessionSetting = serde_json::from_value(setting.clone()).unwrap();
        assert!(matches!(
            decoded.control,
            SessionSettingControl::Unsupported(_)
        ));
        assert_eq!(serde_json::to_value(decoded).unwrap(), setting);
        setting["control"] = json!({"type":"select", "value":"a", "options":[{"value":"a","name":"A","description":null}]});
        serde_json::from_value::<SessionSetting>(setting.clone()).unwrap();
        setting["control"]["typo"] = json!(true);
        assert!(serde_json::from_value::<SessionSetting>(setting).is_err());
    }
}
