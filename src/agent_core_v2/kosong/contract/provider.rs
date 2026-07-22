use serde::{Deserialize, Serialize};
use std::fmt;

// Original:
//   packages/agent-core-v2/src/kosong/contract/provider.ts
//   ThinkingEffort
//
// Rust adaptation:
//   The TypeScript union deliberately accepts arbitrary provider-defined
//   strings. A transparent newtype preserves that open string contract while
//   preventing unrelated strings from being passed accidentally.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThinkingEffort(String);

impl ThinkingEffort {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_off(&self) -> bool {
        self.0 == "off"
    }
}

impl From<&str> for ThinkingEffort {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ThinkingEffort {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for ThinkingEffort {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ThinkingEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// MIGRATION-TODO:
// Original: packages/agent-core-v2/src/kosong/contract/provider.ts
// Missing unit: the remaining response-format, generation, streaming, upload,
// and ChatProvider contracts.
// Temporary behavior: only ThinkingEffort is exported by this module.
// Completion condition: migrate the message/tool/usage contracts, then port
// the remaining provider contract types without changing their wire shapes.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_preserves_open_string_and_json_contract() {
        let effort = ThinkingEffort::from("provider-custom");
        assert_eq!(effort.as_str(), "provider-custom");
        assert_eq!(
            serde_json::to_string(&effort).unwrap(),
            "\"provider-custom\""
        );
        assert_eq!(
            serde_json::from_str::<ThinkingEffort>("\"off\"").unwrap(),
            ThinkingEffort::from("off")
        );
    }
}
