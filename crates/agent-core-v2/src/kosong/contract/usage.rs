use serde::{Deserialize, Serialize};

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   TokenUsage
//
// Rust adaptation:
//   TypeScript exposes unrestricted `number` counters; these are token counts,
//   so u64 replaces f64. Integer-valued JSON floats such as `100.0` written by
//   legacy writers are still accepted via the lenient deserializer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub input_other: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub output: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub input_cache_read: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub input_cache_creation: u64,
}

use serde_json::Value;

/// Converts a JSON counter to `u64`, accepting both `100` and the legacy
/// `100.0` float spelling (truncated). Non-finite, negative, or non-numeric
/// values fall back to 0, matching the original `Number(value) || 0` behavior.
pub fn counter_from_json(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|number| number.is_finite() && *number >= 0.0)
                    .map(|number| number.trunc() as u64)
            })
            .unwrap_or(0),
        _ => 0,
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   inputTotal()
pub fn input_total(usage: &TokenUsage) -> u64 {
    usage
        .input_other
        .saturating_add(usage.input_cache_read)
        .saturating_add(usage.input_cache_creation)
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   grandTotal()
pub fn grand_total(usage: &TokenUsage) -> u64 {
    input_total(usage).saturating_add(usage.output)
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   emptyUsage()
pub fn empty_usage() -> TokenUsage {
    TokenUsage::default()
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   addUsage()
pub fn add_usage(left: &TokenUsage, right: &TokenUsage) -> TokenUsage {
    TokenUsage {
        input_other: left.input_other.saturating_add(right.input_other),
        output: left.output.saturating_add(right.output),
        input_cache_read: left.input_cache_read.saturating_add(right.input_cache_read),
        input_cache_creation: left
            .input_cache_creation
            .saturating_add(right.input_cache_creation),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_usage_is_all_zeros() {
        assert_eq!(
            empty_usage(),
            TokenUsage {
                input_other: 0,
                output: 0,
                input_cache_read: 0,
                input_cache_creation: 0,
            }
        );
    }

    #[test]
    fn add_usage_sums_every_counter() {
        let left = TokenUsage {
            input_other: 1,
            output: 2,
            input_cache_read: 3,
            input_cache_creation: 4,
        };
        let right = TokenUsage {
            input_other: 10,
            output: 20,
            input_cache_read: 30,
            input_cache_creation: 40,
        };
        assert_eq!(
            add_usage(&left, &right),
            TokenUsage {
                input_other: 11,
                output: 22,
                input_cache_read: 33,
                input_cache_creation: 44,
            }
        );
    }

    #[test]
    fn totals_preserve_input_then_output_order() {
        let usage = TokenUsage {
            input_other: 5,
            output: 7,
            input_cache_read: 11,
            input_cache_creation: 13,
        };
        assert_eq!(input_total(&usage), 29);
        assert_eq!(grand_total(&usage), 36);
    }

    #[test]
    fn serialized_shape_preserves_original_field_names() {
        assert_eq!(
            serde_json::to_value(TokenUsage {
                input_other: 1,
                output: 2,
                input_cache_read: 3,
                input_cache_creation: 4,
            })
            .unwrap(),
            serde_json::json!({
                "inputOther": 1,
                "output": 2,
                "inputCacheRead": 3,
                "inputCacheCreation": 4,
            })
        );
    }
}
