use super::types::{CompletionBudgetConfig, CompletionBudgetParams};
use crate::kosong::contract::capability::ModelCapability;

const DEFAULT_UNKNOWN_CONTEXT_FALLBACK: u64 = 32_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolveCompletionBudgetArgs {
    pub max_output_size: Option<u64>,
    pub reserved_context_size: Option<u64>,
    pub max_completion_tokens_cap: Option<u64>,
}

// Original:
//   packages/agent-core-v2/src/kosong/model/completionBudget.ts
//   resolveCompletionBudget()
//
// Rust adaptation:
//   Token counts are u64, so the original `cap <= 0 disables the budget`
//   check becomes `cap == 0`.
pub fn resolve_completion_budget(
    args: ResolveCompletionBudgetArgs,
) -> Option<CompletionBudgetConfig> {
    if let Some(cap) = args.max_completion_tokens_cap {
        if cap == 0 {
            return None;
        }
        return Some(CompletionBudgetConfig {
            hard_cap: Some(cap),
            fallback: None,
        });
    }
    if let Some(max_output_size) = args.max_output_size
        && max_output_size > 0
    {
        return Some(CompletionBudgetConfig {
            hard_cap: Some(max_output_size),
            fallback: None,
        });
    }
    if let Some(reserved_context_size) = args.reserved_context_size
        && reserved_context_size > 0
    {
        return Some(CompletionBudgetConfig {
            hard_cap: None,
            fallback: Some(reserved_context_size),
        });
    }
    Some(CompletionBudgetConfig {
        hard_cap: None,
        fallback: Some(DEFAULT_UNKNOWN_CONTEXT_FALLBACK),
    })
}

// Original:
//   packages/agent-core-v2/src/kosong/model/completionBudget.ts
//   computeCompletionBudgetCap()
//
// Rust adaptation:
//   The original JavaScript `Math.max(1, cap)` floor is `cap.max(1)`; NaN
//   cannot occur with integer token counts.
pub fn compute_completion_budget_cap(
    budget: CompletionBudgetConfig,
    capability: Option<&ModelCapability>,
) -> u64 {
    let max_context_tokens = capability.map_or(0, |capability| capability.max_context_tokens);
    let cap = budget.hard_cap.unwrap_or_else(|| {
        if max_context_tokens > 0 {
            max_context_tokens
        } else {
            budget.fallback.unwrap_or(DEFAULT_UNKNOWN_CONTEXT_FALLBACK)
        }
    });
    cap.max(1)
}

// Original:
//   packages/agent-core-v2/src/kosong/model/completionBudget.ts
//   completionBudgetParams()
pub fn completion_budget_params(
    budget: Option<CompletionBudgetConfig>,
    capability: Option<&ModelCapability>,
    used_context_tokens: Option<u64>,
) -> Option<CompletionBudgetParams> {
    let budget = budget?;
    Some(CompletionBudgetParams {
        max_completion_tokens: compute_completion_budget_cap(budget, capability),
        used_context_tokens,
        max_context_tokens: capability.map(|capability| capability.max_context_tokens),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(max_context_tokens: u64) -> ModelCapability {
        ModelCapability {
            image_in: false,
            video_in: false,
            audio_in: false,
            thinking: false,
            tool_use: true,
            max_context_tokens,
            dynamically_loaded_tools: None,
        }
    }

    #[test]
    fn resolution_preserves_source_precedence() {
        assert_eq!(
            resolve_completion_budget(ResolveCompletionBudgetArgs {
                max_completion_tokens_cap: Some(100),
                max_output_size: Some(200),
                reserved_context_size: Some(300),
            }),
            Some(CompletionBudgetConfig {
                hard_cap: Some(100),
                fallback: None,
            })
        );
        assert_eq!(
            resolve_completion_budget(ResolveCompletionBudgetArgs {
                max_output_size: Some(200),
                reserved_context_size: Some(300),
                ..ResolveCompletionBudgetArgs::default()
            }),
            Some(CompletionBudgetConfig {
                hard_cap: Some(200),
                fallback: None,
            })
        );
        assert_eq!(
            resolve_completion_budget(ResolveCompletionBudgetArgs {
                reserved_context_size: Some(300),
                ..ResolveCompletionBudgetArgs::default()
            }),
            Some(CompletionBudgetConfig {
                hard_cap: None,
                fallback: Some(300),
            })
        );
        assert_eq!(
            resolve_completion_budget(ResolveCompletionBudgetArgs::default()),
            Some(CompletionBudgetConfig {
                hard_cap: None,
                fallback: Some(32_000),
            })
        );
    }

    #[test]
    fn explicit_zero_cap_disables_the_budget() {
        assert_eq!(
            resolve_completion_budget(ResolveCompletionBudgetArgs {
                max_completion_tokens_cap: Some(0),
                max_output_size: Some(200),
                ..ResolveCompletionBudgetArgs::default()
            }),
            None
        );
        assert_eq!(
            resolve_completion_budget(ResolveCompletionBudgetArgs {
                max_output_size: Some(0),
                reserved_context_size: Some(0),
                ..ResolveCompletionBudgetArgs::default()
            }),
            Some(CompletionBudgetConfig {
                hard_cap: None,
                fallback: Some(32_000),
            })
        );
    }

    #[test]
    fn hard_cap_then_context_window_then_fallback_determine_cap() {
        assert_eq!(
            compute_completion_budget_cap(
                CompletionBudgetConfig {
                    hard_cap: Some(50),
                    fallback: None,
                },
                Some(&capability(128_000)),
            ),
            50
        );
        assert_eq!(
            compute_completion_budget_cap(
                CompletionBudgetConfig {
                    hard_cap: None,
                    fallback: Some(300),
                },
                Some(&capability(128_000)),
            ),
            128_000
        );
        assert_eq!(
            compute_completion_budget_cap(
                CompletionBudgetConfig {
                    hard_cap: None,
                    fallback: Some(300),
                },
                Some(&capability(0)),
            ),
            300
        );
        assert_eq!(
            compute_completion_budget_cap(CompletionBudgetConfig::default(), None),
            32_000
        );
    }

    #[test]
    fn cap_is_floored_at_one() {
        assert_eq!(
            compute_completion_budget_cap(
                CompletionBudgetConfig {
                    hard_cap: Some(0),
                    fallback: None,
                },
                None,
            ),
            1
        );
    }

    #[test]
    fn fold_is_absent_without_budget_and_carries_caller_measurement_verbatim() {
        let capability = capability(128_000);
        assert_eq!(
            completion_budget_params(None, Some(&capability), None),
            None
        );
        assert_eq!(
            completion_budget_params(
                Some(CompletionBudgetConfig {
                    hard_cap: Some(8192),
                    fallback: None,
                }),
                Some(&capability),
                Some(5000),
            ),
            Some(CompletionBudgetParams {
                max_completion_tokens: 8192,
                used_context_tokens: Some(5000),
                max_context_tokens: Some(128_000),
            })
        );
    }

    #[test]
    fn fold_omits_measurement_when_caller_overrode_messages() {
        let params = completion_budget_params(
            Some(CompletionBudgetConfig {
                hard_cap: Some(8192),
                fallback: None,
            }),
            Some(&capability(128_000)),
            None,
        )
        .unwrap();
        assert_eq!(params.used_context_tokens, None);
        assert_eq!(params.max_context_tokens, Some(128_000));
        assert_eq!(params.max_completion_tokens, 8192);
    }
}
