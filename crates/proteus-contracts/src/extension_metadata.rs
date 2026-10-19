//! Optional descriptive data. It never changes required protocol semantics.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de::Error};
use serde_json::Value;

/// Keys belong to an author namespace, e.g. `org.example/build`.
/// Unknown entries are preserved, but must not grant capabilities or authority.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExtensionMetadata(pub BTreeMap<String, Value>);

impl ExtensionMetadata {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for ExtensionMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = BTreeMap::<String, Value>::deserialize(deserializer)?;
        for key in values.keys() {
            if !key.split_once('/').is_some_and(|(namespace, name)| {
                !namespace.is_empty() && !name.is_empty() && !key.chars().any(char::is_whitespace)
            }) {
                return Err(D::Error::custom("metadata keys require a namespace/name"));
            }
        }
        Ok(Self(values))
    }
}
