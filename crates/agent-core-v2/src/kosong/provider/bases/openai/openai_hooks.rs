use indexmap::IndexMap;
use serde_json::Value;
use std::sync::Arc;

use crate::kosong::contract::message::Message;
use crate::kosong::contract::provider::{
    GenerateOptions, ThinkingEffort, ToolCallIdPolicy, VideoUploadSource,
};
use crate::kosong::contract::tool::Tool;
use crate::kosong::protocol::protocol_trait::{
    JsonObject, ResolvedTrait, ThinkingHookOptions, UploadVideoFuture, UsageExtraction,
};

pub type BoundConvertMessageHook =
    Arc<dyn Fn(&Message, JsonObject) -> Option<JsonObject> + Send + Sync>;
pub type BoundMergeHistoryHook = Arc<dyn Fn(&[JsonObject]) -> Vec<JsonObject> + Send + Sync>;
pub type BoundBuildParamsHook = Arc<dyn Fn(JsonObject) -> JsonObject + Send + Sync>;
pub type BoundConvertToolHook = Arc<dyn Fn(&Tool) -> Option<JsonObject> + Send + Sync>;
pub type BoundToolCallIdPolicyHook = Arc<dyn Fn() -> Option<ToolCallIdPolicy> + Send + Sync>;
pub type BoundWithThinkingHook = Arc<
    dyn Fn(&ThinkingEffort, &ThinkingHookOptions, &JsonObject) -> Option<JsonObject> + Send + Sync,
>;
pub type BoundPreserveThinkingHook = Arc<dyn Fn(&JsonObject) -> Option<bool> + Send + Sync>;
pub type BoundMaxCompletionTokensHook = Arc<dyn Fn(u64) -> Option<JsonObject> + Send + Sync>;
pub type BoundCacheKeyHook = Arc<dyn Fn(&str) -> Option<JsonObject> + Send + Sync>;
pub type BoundExtractUsageHook = Arc<dyn Fn(&JsonObject) -> UsageExtraction + Send + Sync>;
pub type BoundReasoningKeyHook = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub type BoundUploadVideoHook = Arc<
    dyn Fn(VideoUploadSource, Option<GenerateOptions>) -> UploadVideoFuture<'static> + Send + Sync,
>;

#[derive(Clone, Default)]
pub struct OpenAiChatHooks {
    pub convert_message: Option<BoundConvertMessageHook>,
    pub merge_history: Option<BoundMergeHistoryHook>,
    pub build_params: Option<BoundBuildParamsHook>,
    pub convert_tool: Option<BoundConvertToolHook>,
    pub tool_call_id_policy: Option<BoundToolCallIdPolicyHook>,
    pub with_thinking: Option<BoundWithThinkingHook>,
    pub preserve_thinking: Option<BoundPreserveThinkingHook>,
    pub with_max_completion_tokens: Option<BoundMaxCompletionTokensHook>,
    pub cache_key: Option<BoundCacheKeyHook>,
    pub extract_usage: Option<BoundExtractUsageHook>,
    pub reasoning_key: Option<BoundReasoningKeyHook>,
    pub upload_video: Option<BoundUploadVideoHook>,
}

impl OpenAiChatHooks {
    fn is_empty(&self) -> bool {
        self.convert_message.is_none()
            && self.merge_history.is_none()
            && self.build_params.is_none()
            && self.convert_tool.is_none()
            && self.tool_call_id_policy.is_none()
            && self.with_thinking.is_none()
            && self.preserve_thinking.is_none()
            && self.with_max_completion_tokens.is_none()
            && self.cache_key.is_none()
            && self.extract_usage.is_none()
            && self.reasoning_key.is_none()
            && self.upload_video.is_none()
    }
}

// Original: openaiHooks.ts, composeOpenAIChatHooks()
pub fn compose_openai_chat_hooks(traits: &[ResolvedTrait]) -> Option<OpenAiChatHooks> {
    let mut hooks = OpenAiChatHooks::default();

    let message_shapers = traits
        .iter()
        .filter_map(|resolved| {
            resolved
                .protocol_trait
                .convert_message
                .as_ref()
                .map(|hook| (Arc::clone(hook), resolved.context.clone()))
        })
        .collect::<Vec<_>>();
    if !message_shapers.is_empty() {
        hooks.convert_message = Some(Arc::new(move |message, converted| {
            let mut current = converted;
            for (hook, context) in &message_shapers {
                current = hook(message, current, context)?;
            }
            Some(current)
        }));
    }

    let history_mergers = traits
        .iter()
        .filter_map(|resolved| {
            resolved
                .protocol_trait
                .merge_history
                .as_ref()
                .map(|hook| (Arc::clone(hook), resolved.context.clone()))
        })
        .collect::<Vec<_>>();
    if !history_mergers.is_empty() {
        hooks.merge_history = Some(Arc::new(move |messages| {
            let mut current = messages.to_vec();
            for (hook, context) in &history_mergers {
                if let Some(next) = hook(&current, context) {
                    current = next;
                }
            }
            current
        }));
    }

    let params_builders = traits
        .iter()
        .filter_map(|resolved| {
            resolved
                .protocol_trait
                .build_params
                .as_ref()
                .map(|hook| (Arc::clone(hook), resolved.context.clone()))
        })
        .collect::<Vec<_>>();
    if !params_builders.is_empty() {
        hooks.build_params = Some(Arc::new(move |params| {
            let mut current = params;
            for (hook, context) in &params_builders {
                if let Some(next) = hook(current.clone(), context) {
                    current = next;
                }
            }
            current
        }));
    }

    for resolved in traits {
        let context = resolved.context.clone();
        let protocol_trait = &resolved.protocol_trait;
        if let Some(hook) = protocol_trait.convert_tool.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.convert_tool = Some(Arc::new(move |tool| hook(tool, &context)));
        }
        if let Some(hook) = protocol_trait.tool_call_id_policy.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.tool_call_id_policy = Some(Arc::new(move || hook(&context)));
        }
        if let Some(hook) = protocol_trait.with_thinking.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.with_thinking = Some(Arc::new(move |effort, options, kwargs| {
                hook(effort, options, kwargs, &context)
            }));
        }
        if let Some(hook) = protocol_trait.preserve_thinking.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.preserve_thinking = Some(Arc::new(move |kwargs| hook(kwargs, &context)));
        }
        if let Some(hook) = protocol_trait.with_max_completion_tokens.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.with_max_completion_tokens = Some(Arc::new(move |tokens| hook(tokens, &context)));
        }
        if let Some(hook) = protocol_trait.cache_key.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.cache_key = Some(Arc::new(move |key| hook(key, &context)));
        }
        if let Some(hook) = protocol_trait.extract_usage.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.extract_usage = Some(Arc::new(move |chunk| hook(chunk, &context)));
        }
        if let Some(hook) = protocol_trait.reasoning_key.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.reasoning_key = Some(Arc::new(move || hook(&context)));
        }
        if let Some(hook) = protocol_trait.upload_video.as_ref() {
            let hook = Arc::clone(hook);
            let context = context.clone();
            hooks.upload_video = Some(Arc::new(move |input, options| {
                let hook = Arc::clone(&hook);
                let context = context.clone();
                Box::pin(async move { hook(&input, options.as_ref(), &context).await })
            }));
        }
    }

    (!hooks.is_empty()).then_some(hooks)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregatedEndpoint {
    pub api_key_env: Vec<String>,
    pub base_url_env: Vec<String>,
    pub default_base_url: Option<String>,
}

// Original: openaiHooks.ts, traitEndpoint()
pub fn trait_endpoint(traits: &[ResolvedTrait]) -> Option<AggregatedEndpoint> {
    let mut endpoint = AggregatedEndpoint {
        api_key_env: Vec::new(),
        base_url_env: Vec::new(),
        default_base_url: None,
    };
    let mut declared = false;
    for resolved in traits {
        let Some(hook) = resolved.protocol_trait.endpoint.as_ref() else {
            continue;
        };
        let Some(next) = hook(&resolved.context) else {
            continue;
        };
        declared = true;
        endpoint.api_key_env.extend(next.api_key_env);
        endpoint.base_url_env.extend(next.base_url_env);
        if next.default_base_url.is_some() {
            endpoint.default_base_url = next.default_base_url;
        }
    }
    declared.then_some(endpoint)
}

pub fn first_env(names: Option<&[String]>, env: &IndexMap<String, String>) -> Option<String> {
    names?
        .iter()
        .find_map(|name| env.get(name).filter(|value| !value.is_empty()).cloned())
}

// Original: openaiHooks.ts, firstProcessEnv()
pub fn first_process_env(names: Option<&[String]>) -> Option<String> {
    names?
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

// Original: openaiHooks.ts, traitProvides()
pub fn trait_provides(traits: &[ResolvedTrait]) -> Option<JsonObject> {
    let mut provides: Option<JsonObject> = None;
    for resolved in traits {
        let Some(hook) = resolved.protocol_trait.provides.as_ref() else {
            continue;
        };
        let Some(declared) = hook(&resolved.context) else {
            continue;
        };
        provides.get_or_insert_default().extend(declared);
    }
    provides
}

// Original: openaiHooks.ts, compactObject()
pub fn compact_object(values: IndexMap<String, Option<Value>>) -> JsonObject {
    values
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::{ContentPart, Role};
    use crate::kosong::protocol::identity::{Protocol, ProtocolAdapterConfig};
    use crate::kosong::protocol::protocol_trait::{ProtocolEndpoint, ProtocolTrait, TraitContext};
    use serde_json::{Map, json};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn resolved(protocol_trait: ProtocolTrait) -> ResolvedTrait {
        ResolvedTrait {
            protocol_trait: Arc::new(protocol_trait),
            context: TraitContext {
                config: ProtocolAdapterConfig {
                    protocol: Protocol::OpenAi,
                    provider_type: None,
                    base_url: None,
                    model_name: "m".to_owned(),
                    api_key: None,
                    default_headers: None,
                    provider_options: None,
                },
                provider_id: None,
            },
        }
    }

    fn user_message() -> Message {
        Message::new(
            Role::User,
            vec![ContentPart::Text {
                text: "hi".to_owned(),
            }],
            Vec::new(),
        )
    }

    #[test]
    fn message_pipeline_chains_in_order_and_short_circuits_on_drop() {
        let second_called = Arc::new(AtomicBool::new(false));
        let called = Arc::clone(&second_called);
        let hooks = compose_openai_chat_hooks(&[
            resolved(ProtocolTrait {
                convert_message: Some(Arc::new(|_, mut converted, _| {
                    converted.insert("first".to_owned(), Value::Bool(true));
                    Some(converted)
                })),
                ..ProtocolTrait::default()
            }),
            resolved(ProtocolTrait {
                convert_message: Some(Arc::new(move |_, mut converted, _| {
                    called.store(true, Ordering::SeqCst);
                    converted.insert("second".to_owned(), converted["first"].clone());
                    Some(converted)
                })),
                ..ProtocolTrait::default()
            }),
        ])
        .unwrap();
        let out = hooks.convert_message.unwrap()(&user_message(), Map::new()).unwrap();
        assert_eq!(
            out,
            json!({"first": true, "second": true})
                .as_object()
                .unwrap()
                .clone()
        );
        assert!(second_called.load(Ordering::SeqCst));

        let never_called = Arc::new(AtomicBool::new(false));
        let called = Arc::clone(&never_called);
        let hooks = compose_openai_chat_hooks(&[
            resolved(ProtocolTrait {
                convert_message: Some(Arc::new(|_, _, _| None)),
                ..ProtocolTrait::default()
            }),
            resolved(ProtocolTrait {
                convert_message: Some(Arc::new(move |_, converted, _| {
                    called.store(true, Ordering::SeqCst);
                    Some(converted)
                })),
                ..ProtocolTrait::default()
            }),
        ])
        .unwrap();
        assert!(hooks.convert_message.unwrap()(&user_message(), Map::new()).is_none());
        assert!(!never_called.load(Ordering::SeqCst));
    }

    #[test]
    fn history_and_params_pipelines_keep_previous_value_on_no_override() {
        let hooks = compose_openai_chat_hooks(&[
            resolved(ProtocolTrait {
                merge_history: Some(Arc::new(|messages, _| {
                    let mut out = messages.to_vec();
                    out.push(Map::from_iter([(
                        "marker".to_owned(),
                        Value::String("a".to_owned()),
                    )]));
                    Some(out)
                })),
                build_params: Some(Arc::new(|mut params, _| {
                    params.insert("a".to_owned(), Value::from(1));
                    Some(params)
                })),
                ..ProtocolTrait::default()
            }),
            resolved(ProtocolTrait {
                merge_history: Some(Arc::new(|_, _| None)),
                build_params: Some(Arc::new(|mut params, _| {
                    params.insert(
                        "b".to_owned(),
                        Value::from(params["a"].as_i64().unwrap() + 1),
                    );
                    Some(params)
                })),
                ..ProtocolTrait::default()
            }),
        ])
        .unwrap();
        assert_eq!(hooks.merge_history.unwrap()(&[Map::new()]).len(), 2);
        assert_eq!(
            hooks.build_params.unwrap()(Map::new()),
            json!({"a":1,"b":2}).as_object().unwrap().clone()
        );
    }

    #[test]
    fn last_single_value_declarer_wins_and_construction_only_traits_are_ignored() {
        let hooks = compose_openai_chat_hooks(&[
            resolved(ProtocolTrait {
                cache_key: Some(Arc::new(|key, _| {
                    Some(Map::from_iter([(
                        "first".to_owned(),
                        Value::String(key.to_owned()),
                    )]))
                })),
                reasoning_key: Some(Arc::new(|_| Some("first".to_owned()))),
                ..ProtocolTrait::default()
            }),
            resolved(ProtocolTrait {
                cache_key: Some(Arc::new(|key, _| {
                    Some(Map::from_iter([(
                        "prompt_cache_key".to_owned(),
                        Value::String(key.to_owned()),
                    )]))
                })),
                reasoning_key: Some(Arc::new(|_| Some("reasoning_content".to_owned()))),
                ..ProtocolTrait::default()
            }),
        ])
        .unwrap();
        assert_eq!(
            hooks.reasoning_key.unwrap()().as_deref(),
            Some("reasoning_content")
        );
        assert!(
            hooks.cache_key.unwrap()("session-1")
                .unwrap()
                .contains_key("prompt_cache_key")
        );
        assert!(
            compose_openai_chat_hooks(&[resolved(ProtocolTrait {
                endpoint: Some(Arc::new(|_| Some(ProtocolEndpoint::default()))),
                provides: Some(Arc::new(|_| Some(Map::new()))),
                ..ProtocolTrait::default()
            })])
            .is_none()
        );
    }

    #[test]
    fn endpoint_provides_env_and_compaction_preserve_source_ordering_rules() {
        let traits = [
            resolved(ProtocolTrait {
                endpoint: Some(Arc::new(|_| {
                    Some(ProtocolEndpoint {
                        api_key_env: Some("A".to_owned()),
                        base_url_env: Some("A_URL".to_owned()),
                        default_base_url: Some("first".to_owned()),
                    })
                })),
                provides: Some(Arc::new(|_| {
                    Some(Map::from_iter([
                        ("a".to_owned(), Value::from(1)),
                        ("stream".to_owned(), Value::Bool(false)),
                    ]))
                })),
                ..ProtocolTrait::default()
            }),
            resolved(ProtocolTrait {
                endpoint: Some(Arc::new(|_| {
                    Some(ProtocolEndpoint {
                        api_key_env: Some("B".to_owned()),
                        base_url_env: Some("B_URL".to_owned()),
                        default_base_url: Some("second".to_owned()),
                    })
                })),
                provides: Some(Arc::new(|_| {
                    Some(Map::from_iter([("a".to_owned(), Value::from(2))]))
                })),
                ..ProtocolTrait::default()
            }),
        ];
        assert_eq!(
            trait_endpoint(&traits).unwrap(),
            AggregatedEndpoint {
                api_key_env: vec!["A".to_owned(), "B".to_owned()],
                base_url_env: vec!["A_URL".to_owned(), "B_URL".to_owned()],
                default_base_url: Some("second".to_owned())
            }
        );
        assert_eq!(
            first_env(
                Some(&["A".to_owned(), "B".to_owned()]),
                &IndexMap::from([
                    ("A".to_owned(), String::new()),
                    ("B".to_owned(), "hit".to_owned())
                ])
            )
            .as_deref(),
            Some("hit")
        );
        assert_eq!(trait_provides(&traits).unwrap()["a"], 2);
        assert_eq!(
            compact_object(IndexMap::from([
                ("a".to_owned(), None),
                ("b".to_owned(), Some(Value::from(1)))
            ])),
            Map::from_iter([("b".to_owned(), Value::from(1))])
        );
    }
}
