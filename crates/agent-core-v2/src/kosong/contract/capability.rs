use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapability {
    pub image_in: bool,
    pub video_in: bool,
    pub audio_in: bool,
    pub thinking: bool,
    pub tool_use: bool,
    pub max_context_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamically_loaded_tools: Option<bool>,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/capability.ts
//   UNKNOWN_CAPABILITY
pub const UNKNOWN_CAPABILITY: ModelCapability = ModelCapability {
    image_in: false,
    video_in: false,
    audio_in: false,
    thinking: false,
    tool_use: false,
    max_context_tokens: 0,
    dynamically_loaded_tools: Some(false),
};

// Original:
//   packages/agent-core-v2/src/kosong/contract/capability.ts
//   isUnknownCapability()
//
// Rust adaptation: TypeScript also carries a non-serialized Symbol marker on
// its frozen singleton. Rust's constant has value semantics, so the same
// observable structural predicate covers both the constant and deserialized
// or copied unknown capabilities.
pub fn is_unknown_capability(capability: &ModelCapability) -> bool {
    !capability.image_in
        && !capability.video_in
        && !capability.audio_in
        && !capability.thinking
        && !capability.tool_use
        && capability.dynamically_loaded_tools != Some(true)
        && capability.max_context_tokens == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_preserves_source_unknown_shape() {
        assert!(is_unknown_capability(&UNKNOWN_CAPABILITY));
        assert_eq!(
            serde_json::to_value(&UNKNOWN_CAPABILITY).unwrap(),
            serde_json::json!({
                "image_in": false,
                "video_in": false,
                "audio_in": false,
                "thinking": false,
                "tool_use": false,
                "max_context_tokens": 0,
                "dynamically_loaded_tools": false,
            })
        );
    }

    #[test]
    fn structurally_empty_capability_is_unknown_with_missing_or_false_dynamic_tools() {
        let mut capability = UNKNOWN_CAPABILITY.clone();
        capability.dynamically_loaded_tools = None;
        assert!(is_unknown_capability(&capability));
        capability.dynamically_loaded_tools = Some(false);
        assert!(is_unknown_capability(&capability));
    }

    #[test]
    fn every_positive_capability_signal_makes_it_known() {
        let variants = [
            ModelCapability {
                image_in: true,
                ..UNKNOWN_CAPABILITY
            },
            ModelCapability {
                video_in: true,
                ..UNKNOWN_CAPABILITY
            },
            ModelCapability {
                audio_in: true,
                ..UNKNOWN_CAPABILITY
            },
            ModelCapability {
                thinking: true,
                ..UNKNOWN_CAPABILITY
            },
            ModelCapability {
                tool_use: true,
                ..UNKNOWN_CAPABILITY
            },
            ModelCapability {
                max_context_tokens: 128_000,
                ..UNKNOWN_CAPABILITY
            },
            ModelCapability {
                dynamically_loaded_tools: Some(true),
                ..UNKNOWN_CAPABILITY
            },
        ];
        for capability in variants {
            assert!(!is_unknown_capability(&capability), "{capability:?}");
        }
    }
}
