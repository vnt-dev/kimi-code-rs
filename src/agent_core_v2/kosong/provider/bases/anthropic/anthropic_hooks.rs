use std::sync::Arc;

use crate::agent_core_v2::kosong::contract::provider::ThinkingEffort;
use crate::agent_core_v2::kosong::protocol::protocol_trait::{
    JsonObject, ResolvedTrait, ThinkingHookOptions,
};

pub type AnthropicWithThinkingHook = Arc<
    dyn Fn(&ThinkingEffort, &ThinkingHookOptions, &JsonObject) -> Option<JsonObject> + Send + Sync,
>;

#[derive(Clone)]
pub struct AnthropicHooks {
    pub with_thinking: AnthropicWithThinkingHook,
}

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/anthropic/anthropicHooks.ts
//   composeAnthropicHooks()
pub fn compose_anthropic_hooks(traits: &[ResolvedTrait]) -> Option<AnthropicHooks> {
    let resolved = traits
        .iter()
        .rev()
        .find(|resolved| resolved.protocol_trait.with_thinking.is_some())?;
    let hook = Arc::clone(resolved.protocol_trait.with_thinking.as_ref()?);
    let context = resolved.context.clone();
    Some(AnthropicHooks {
        with_thinking: Arc::new(move |effort, options, kwargs| {
            let kwargs = kwargs.clone();
            hook(effort, options, &kwargs, &context)
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::protocol::identity::{Protocol, ProtocolAdapterConfig};
    use crate::agent_core_v2::kosong::protocol::protocol_trait::{ProtocolTrait, TraitContext};
    use serde_json::{Map, Value};

    fn resolved(protocol_trait: ProtocolTrait, provider_id: &str) -> ResolvedTrait {
        ResolvedTrait {
            protocol_trait: Arc::new(protocol_trait),
            context: TraitContext {
                config: ProtocolAdapterConfig {
                    protocol: Protocol::Anthropic,
                    provider_type: Some(provider_id.to_owned()),
                    base_url: None,
                    model_name: "model".to_owned(),
                    api_key: None,
                    default_headers: None,
                    provider_options: None,
                },
                provider_id: Some(provider_id.to_owned()),
            },
        }
    }

    #[test]
    fn returns_none_without_a_thinking_hook() {
        assert!(compose_anthropic_hooks(&[]).is_none());
        assert!(compose_anthropic_hooks(&[resolved(ProtocolTrait::default(), "vendor")]).is_none());
    }

    #[test]
    fn last_thinking_declarer_wins_and_receives_bound_context() {
        let hooks = compose_anthropic_hooks(&[
            resolved(
                ProtocolTrait {
                    with_thinking: Some(Arc::new(|_, _, _kwargs, _| {
                        Some(Map::from_iter([(
                            "winner".to_owned(),
                            Value::String("first".to_owned()),
                        )]))
                    })),
                    ..ProtocolTrait::default()
                },
                "first-vendor",
            ),
            resolved(
                ProtocolTrait {
                    with_thinking: Some(Arc::new(|_, _, kwargs, context| {
                        let mut out = kwargs.clone();
                        out.insert(
                            "winner".to_owned(),
                            Value::String(context.provider_id.clone().unwrap()),
                        );
                        Some(out)
                    })),
                    ..ProtocolTrait::default()
                },
                "last-vendor",
            ),
        ])
        .unwrap();
        let seeded = Map::from_iter([("seeded".to_owned(), Value::from(1))]);
        let out = (hooks.with_thinking)(
            &ThinkingEffort::from("high"),
            &ThinkingHookOptions::default(),
            &seeded,
        )
        .unwrap();
        assert_eq!(out["seeded"], 1);
        assert_eq!(out["winner"], "last-vendor");
        assert_eq!(seeded.len(), 1);
    }
}
