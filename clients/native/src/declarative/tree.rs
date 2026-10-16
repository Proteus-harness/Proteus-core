use super::widget::Tone;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub expanded: Option<bool>,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub selected: bool,
}
