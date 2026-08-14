use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::message::{ContentPart, Message};
use crate::kosong::contract::provider::{
    GenerateOptions, ProviderError, ThinkingEffort, ToolCallIdPolicy, VideoUploadSource,
};
use crate::kosong::contract::tool::Tool;

use super::identity::ProtocolAdapterConfig;

pub type JsonObject = Map<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub struct TraitContext {
    pub config: ProtocolAdapterConfig,
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtocolEndpoint {
    pub api_key_env: Option<String>,
    pub base_url_env: Option<String>,
    pub default_base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThinkingHookOptions {
    pub keep: Option<String>,
}

// Original: protocolTrait.ts, extractUsage() return contract.
// The enum preserves JavaScript's three distinct results: undefined means
// defer to the base, null suppresses base extraction, and an object supplies
// the raw usage payload.
#[derive(Debug, Clone, PartialEq)]
pub enum UsageExtraction {
    Defer,
    NoUsage,
    Usage(JsonObject),
}

pub type ProvidesHook = Arc<dyn Fn(&TraitContext) -> Option<JsonObject> + Send + Sync>;
pub type EndpointHook = Arc<dyn Fn(&TraitContext) -> Option<ProtocolEndpoint> + Send + Sync>;
pub type DefaultHeadersHook =
    Arc<dyn Fn(&TraitContext) -> Option<IndexMap<String, String>> + Send + Sync>;
pub type ConvertToolHook = Arc<dyn Fn(&Tool, &TraitContext) -> Option<JsonObject> + Send + Sync>;
pub type ConvertMessageHook =
    Arc<dyn Fn(&Message, JsonObject, &TraitContext) -> Option<JsonObject> + Send + Sync>;
pub type MergeHistoryHook =
    Arc<dyn Fn(&[JsonObject], &TraitContext) -> Option<Vec<JsonObject>> + Send + Sync>;
pub type BuildParamsHook =
    Arc<dyn Fn(JsonObject, &TraitContext) -> Option<JsonObject> + Send + Sync>;
pub type ToolCallIdPolicyHook =
    Arc<dyn Fn(&TraitContext) -> Option<ToolCallIdPolicy> + Send + Sync>;
pub type WithThinkingHook = Arc<
    dyn Fn(&ThinkingEffort, &ThinkingHookOptions, &JsonObject, &TraitContext) -> Option<JsonObject>
        + Send
        + Sync,
>;
pub type PreserveThinkingHook =
    Arc<dyn Fn(&JsonObject, &TraitContext) -> Option<bool> + Send + Sync>;
pub type WithMaxCompletionTokensHook =
    Arc<dyn Fn(u64, &TraitContext) -> Option<JsonObject> + Send + Sync>;
pub type CacheKeyHook = Arc<dyn Fn(&str, &TraitContext) -> Option<JsonObject> + Send + Sync>;
pub type ExtractUsageHook =
    Arc<dyn Fn(&JsonObject, &TraitContext) -> UsageExtraction + Send + Sync>;
pub type ReasoningKeyHook = Arc<dyn Fn(&TraitContext) -> Option<String> + Send + Sync>;
pub type CapabilityHook = Arc<dyn Fn(&str, &TraitContext) -> Option<ModelCapability> + Send + Sync>;
pub type UploadVideoFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ContentPart, ProviderError>> + Send + 'a>>;
pub type UploadVideoHook = Arc<
    dyn for<'a> Fn(
            &'a VideoUploadSource,
            Option<&'a GenerateOptions>,
            &'a TraitContext,
        ) -> UploadVideoFuture<'a>
        + Send
        + Sync,
>;

// Original:
//   packages/agent-core-v2/src/kosong/protocol/protocolTrait.ts
//   ProtocolTrait
//
// Rust adaptation:
//   A data struct of optional callbacks retains the observable difference
//   between an undeclared hook and a hook that returns no override. Arc keeps
//   resolved declarations immutable and cheaply shareable without adding
//   locks or changing invocation order.
#[derive(Clone, Default)]
pub struct ProtocolTrait {
    pub strict_thinking_validation: Option<bool>,
    pub provides: Option<ProvidesHook>,
    pub endpoint: Option<EndpointHook>,
    pub default_headers: Option<DefaultHeadersHook>,
    pub convert_tool: Option<ConvertToolHook>,
    pub convert_message: Option<ConvertMessageHook>,
    pub merge_history: Option<MergeHistoryHook>,
    pub build_params: Option<BuildParamsHook>,
    pub tool_call_id_policy: Option<ToolCallIdPolicyHook>,
    pub with_thinking: Option<WithThinkingHook>,
    pub preserve_thinking: Option<PreserveThinkingHook>,
    pub with_max_completion_tokens: Option<WithMaxCompletionTokensHook>,
    pub cache_key: Option<CacheKeyHook>,
    pub extract_usage: Option<ExtractUsageHook>,
    pub reasoning_key: Option<ReasoningKeyHook>,
    pub capability: Option<CapabilityHook>,
    pub upload_video: Option<UploadVideoHook>,
}

#[derive(Clone)]
pub struct ResolvedTrait {
    pub protocol_trait: Arc<ProtocolTrait>,
    pub context: TraitContext,
}

// Original: protocolTrait.ts, traitDefaultHeaders()
pub fn trait_default_headers(traits: &[ResolvedTrait]) -> Option<IndexMap<String, String>> {
    let mut headers: Option<IndexMap<String, String>> = None;
    for resolved in traits {
        let Some(hook) = resolved.protocol_trait.default_headers.as_ref() else {
            continue;
        };
        let Some(declared) = hook(&resolved.context) else {
            continue;
        };
        headers.get_or_insert_default().extend(declared);
    }
    headers
}

#[cfg(test)]
impl ProtocolTrait {
    fn declared_hook_count(&self) -> usize {
        [
            self.provides.is_some(),
            self.endpoint.is_some(),
            self.default_headers.is_some(),
            self.convert_tool.is_some(),
            self.convert_message.is_some(),
            self.merge_history.is_some(),
            self.build_params.is_some(),
            self.tool_call_id_policy.is_some(),
            self.with_thinking.is_some(),
            self.preserve_thinking.is_some(),
            self.with_max_completion_tokens.is_some(),
            self.cache_key.is_some(),
            self.extract_usage.is_some(),
            self.reasoning_key.is_some(),
            self.capability.is_some(),
            self.upload_video.is_some(),
        ]
        .into_iter()
        .filter(|declared| *declared)
        .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::protocol::identity::Protocol;
    use std::io;
    use parking_lot::Mutex;

    fn context() -> TraitContext {
        TraitContext {
            config: ProtocolAdapterConfig {
                protocol: Protocol::OpenAi,
                provider_type: None,
                base_url: None,
                model_name: "test-model".to_owned(),
                api_key: None,
                default_headers: None,
                provider_options: None,
            },
            provider_id: Some("vendor-x".to_owned()),
        }
    }

    fn resolved(protocol_trait: ProtocolTrait) -> ResolvedTrait {
        ResolvedTrait {
            protocol_trait: Arc::new(protocol_trait),
            context: context(),
        }
    }

    fn unused_upload<'a>(
        _input: &'a VideoUploadSource,
        _options: Option<&'a GenerateOptions>,
        _context: &'a TraitContext,
    ) -> UploadVideoFuture<'a> {
        Box::pin(async { Err(Box::new(io::Error::other("unused")) as ProviderError) })
    }

    #[test]
    fn declares_exactly_the_sixteen_optional_hooks() {
        let protocol_trait = ProtocolTrait {
            provides: Some(Arc::new(|_| None)),
            endpoint: Some(Arc::new(|_| None)),
            default_headers: Some(Arc::new(|_| None)),
            convert_tool: Some(Arc::new(|_, _| None)),
            convert_message: Some(Arc::new(|_, converted, _| Some(converted))),
            merge_history: Some(Arc::new(|_, _| None)),
            build_params: Some(Arc::new(|_, _| None)),
            tool_call_id_policy: Some(Arc::new(|_| None)),
            with_thinking: Some(Arc::new(|_, _, _, _| None)),
            preserve_thinking: Some(Arc::new(|_, _| None)),
            with_max_completion_tokens: Some(Arc::new(|_, _| None)),
            cache_key: Some(Arc::new(|_, _| None)),
            extract_usage: Some(Arc::new(|_, _| UsageExtraction::Defer)),
            reasoning_key: Some(Arc::new(|_| None)),
            capability: Some(Arc::new(|_, _| None)),
            upload_video: Some(Arc::new(unused_upload)),
            ..ProtocolTrait::default()
        };
        assert_eq!(protocol_trait.declared_hook_count(), 16);
        assert_eq!(ProtocolTrait::default().declared_hook_count(), 0);
    }

    #[test]
    fn header_aggregation_returns_none_when_nothing_is_declared() {
        assert!(trait_default_headers(&[]).is_none());
        assert!(trait_default_headers(&[resolved(ProtocolTrait::default())]).is_none());
        assert!(
            trait_default_headers(&[resolved(ProtocolTrait {
                default_headers: Some(Arc::new(|_| None)),
                ..ProtocolTrait::default()
            })])
            .is_none()
        );
    }

    #[test]
    fn header_aggregation_runs_in_order_and_later_values_win() {
        let first = ProtocolTrait {
            default_headers: Some(Arc::new(|_| {
                Some(IndexMap::from([
                    ("x-a".to_owned(), "first".to_owned()),
                    ("x-b".to_owned(), "first".to_owned()),
                ]))
            })),
            ..ProtocolTrait::default()
        };
        let second = ProtocolTrait {
            default_headers: Some(Arc::new(|_| {
                Some(IndexMap::from([
                    ("x-b".to_owned(), "second".to_owned()),
                    ("x-c".to_owned(), "second".to_owned()),
                ]))
            })),
            ..ProtocolTrait::default()
        };
        assert_eq!(
            trait_default_headers(&[resolved(first), resolved(second)]),
            Some(IndexMap::from([
                ("x-a".to_owned(), "first".to_owned()),
                ("x-b".to_owned(), "second".to_owned()),
                ("x-c".to_owned(), "second".to_owned()),
            ]))
        );
    }

    #[test]
    fn header_hook_receives_its_bound_context() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_hook = Arc::clone(&seen);
        let protocol_trait = ProtocolTrait {
            default_headers: Some(Arc::new(move |context| {
                seen_by_hook
                    .lock()
                    .push(context.provider_id.clone());
                Some(IndexMap::from([("x-a".to_owned(), "1".to_owned())]))
            })),
            ..ProtocolTrait::default()
        };
        let entry = resolved(protocol_trait);
        assert_eq!(
            trait_default_headers(&[entry]),
            Some(IndexMap::from([("x-a".to_owned(), "1".to_owned())]))
        );
        assert_eq!(*seen.lock(), vec![Some("vendor-x".to_owned())]);
    }

    #[test]
    fn usage_extraction_keeps_defer_null_and_object_distinct() {
        assert_ne!(UsageExtraction::Defer, UsageExtraction::NoUsage);
        assert_eq!(
            UsageExtraction::Usage(Map::from_iter([("total".to_owned(), Value::from(3))])),
            UsageExtraction::Usage(Map::from_iter([("total".to_owned(), Value::from(3))]))
        );
    }
}
