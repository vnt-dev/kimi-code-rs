use serde::{Deserialize, Serialize};

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   TokenUsage
//
// Rust adaptation:
//   TypeScript exposes unrestricted `number` counters. Using f64 preserves
//   its addition, infinity, and NaN behavior instead of introducing integer
//   overflow or rejecting values accepted by the original interface.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_other: f64,
    pub output: f64,
    pub input_cache_read: f64,
    pub input_cache_creation: f64,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   inputTotal()
pub fn input_total(usage: &TokenUsage) -> f64 {
    usage.input_other + usage.input_cache_read + usage.input_cache_creation
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/usage.ts
//   grandTotal()
pub fn grand_total(usage: &TokenUsage) -> f64 {
    input_total(usage) + usage.output
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
        input_other: left.input_other + right.input_other,
        output: left.output + right.output,
        input_cache_read: left.input_cache_read + right.input_cache_read,
        input_cache_creation: left.input_cache_creation + right.input_cache_creation,
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
                input_other: 0.0,
                output: 0.0,
                input_cache_read: 0.0,
                input_cache_creation: 0.0,
            }
        );
    }

    #[test]
    fn add_usage_sums_every_counter() {
        let left = TokenUsage {
            input_other: 1.0,
            output: 2.0,
            input_cache_read: 3.0,
            input_cache_creation: 4.0,
        };
        let right = TokenUsage {
            input_other: 10.0,
            output: 20.0,
            input_cache_read: 30.0,
            input_cache_creation: 40.0,
        };
        assert_eq!(
            add_usage(&left, &right),
            TokenUsage {
                input_other: 11.0,
                output: 22.0,
                input_cache_read: 33.0,
                input_cache_creation: 44.0,
            }
        );
    }

    #[test]
    fn totals_preserve_input_then_output_order() {
        let usage = TokenUsage {
            input_other: 5.0,
            output: 7.0,
            input_cache_read: 11.0,
            input_cache_creation: 13.0,
        };
        assert_eq!(input_total(&usage), 29.0);
        assert_eq!(grand_total(&usage), 36.0);
    }

    #[test]
    fn serialized_shape_preserves_original_field_names() {
        assert_eq!(
            serde_json::to_value(TokenUsage {
                input_other: 1.0,
                output: 2.0,
                input_cache_read: 3.0,
                input_cache_creation: 4.0,
            })
            .unwrap(),
            serde_json::json!({
                "inputOther": 1.0,
                "output": 2.0,
                "inputCacheRead": 3.0,
                "inputCacheCreation": 4.0,
            })
        );
    }

    #[test]
    fn arithmetic_preserves_javascript_special_number_behavior() {
        let sum = add_usage(
            &TokenUsage {
                input_other: f64::INFINITY,
                output: f64::NAN,
                ..TokenUsage::default()
            },
            &TokenUsage::default(),
        );
        assert!(sum.input_other.is_infinite());
        assert!(sum.output.is_nan());
    }
}
