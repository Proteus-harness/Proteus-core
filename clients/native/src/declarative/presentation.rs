//! Compact native presentation supplied by an extension process.

use anyhow::{Result, ensure};
use serde::Deserialize;

use super::widget::Tone;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Face {
    Icon {
        #[serde(default)]
        count: Option<usize>,
        #[serde(default)]
        tone: Tone,
    },
    Text {
        text: String,
        #[serde(default)]
        icon: bool,
        #[serde(default)]
        caret: bool,
        #[serde(default)]
        tone: Tone,
    },
    Ring {
        percent: f32,
        #[serde(default)]
        tone: Tone,
    },
}

impl Face {
    pub fn validate(&self) -> Result<()> {
        if let Self::Ring { percent, .. } = self {
            ensure!(
                percent.is_finite() && (0. ..=100.).contains(percent),
                "ring percent must be in 0..=100"
            );
        }
        Ok(())
    }

    pub fn narrow(&self) -> bool {
        !matches!(self, Self::Text { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    pub hidden: bool,
    pub title: String,
    pub lines: Vec<String>,
    pub face: Face,
}
