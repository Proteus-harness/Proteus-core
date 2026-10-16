use super::widget::Tone;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub subtitle: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub dimmed: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub label: String,
    pub value: serde_json::Value,
    pub selected: bool,
}
