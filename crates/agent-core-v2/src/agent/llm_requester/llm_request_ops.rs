use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    kosong::contract::provider::ThinkingEffort,
    wire::{
        model::{ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequestToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequestTraceState {
    pub seen_tools_hashes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmToolsSnapshotPayload {
    pub hash: String,
    pub tools: Vec<LlmRequestToolSchema>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmRequestKind {
    Loop,
    Compaction,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlmRequestProjection {
    Strict,
    MediaDegraded,
    MediaStripped,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequestPayload {
    pub kind: LlmRequestKind,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<ThinkingEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_keep: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_lenient_u64"
    )]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta_api: Option<bool>,
    pub tool_select: bool,
    pub system_prompt_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub tools_hash: String,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub message_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<LlmRequestProjection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64"
    )]
    pub dropped_count: Option<u64>,
}

// Legacy wire journals wrote maxTokens as f64 (e.g. `131072.0`); accept
// integer-valued floats so those records still replay after the u64 switch.
fn deserialize_lenient_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<f64>::deserialize(deserializer)?;
    Ok(value.map(|value| value.max(0.0) as u64))
}

pub static LLM_REQUEST_TRACE_MODEL: LazyLock<ModelDef<LlmRequestTraceState>> =
    LazyLock::new(|| {
        define_model(
            "llm.requestTrace",
            LlmRequestTraceState::default,
            ModelOptions::default(),
        )
    });

pub static LLM_TOOLS_SNAPSHOT: LazyLock<DefinedOp<LlmRequestTraceState, LlmToolsSnapshotPayload>> =
    LazyLock::new(|| {
        LLM_REQUEST_TRACE_MODEL
            .define_op(
                "llm.tools_snapshot",
                DefineOpOptions::new(apply_tools_snapshot),
            )
            .expect("llm.tools_snapshot must have one global definition")
    });

pub static LLM_REQUEST: LazyLock<DefinedOp<LlmRequestTraceState, LlmRequestPayload>> =
    LazyLock::new(|| {
        LLM_REQUEST_TRACE_MODEL
            .define_op("llm.request", DefineOpOptions::new(apply_llm_request))
            .expect("llm.request must have one global definition")
    });

// Original: llmRequestOps.ts, llmToolsSnapshot.apply().
fn apply_tools_snapshot(
    mut state: LlmRequestTraceState,
    payload: &LlmToolsSnapshotPayload,
) -> LlmRequestTraceState {
    if !state.seen_tools_hashes.contains(&payload.hash) {
        state.seen_tools_hashes.push(payload.hash.clone());
    }
    state
}

// Original: llmRequestOps.ts, llmRequest.apply().
fn apply_llm_request(
    state: LlmRequestTraceState,
    _payload: &LlmRequestPayload,
) -> LlmRequestTraceState {
    state
}

pub fn llm_tools_snapshot(payload: LlmToolsSnapshotPayload) -> Result<Op, serde_json::Error> {
    LLM_TOOLS_SNAPSHOT.create(payload)
}

pub fn llm_request(payload: LlmRequestPayload) -> Result<Op, serde_json::Error> {
    LLM_REQUEST.create(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::op::{Op, registered_op};

    fn tool(name: &str) -> LlmRequestToolSchema {
        LlmRequestToolSchema {
            name: name.into(),
            description: format!("{name} description"),
            parameters: Map::from_iter([("type".into(), Value::String("object".into()))]),
        }
    }

    fn request() -> LlmRequestPayload {
        LlmRequestPayload {
            kind: LlmRequestKind::Loop,
            provider: "openai".into(),
            model: "kimi-k2".into(),
            model_alias: None,
            thinking_effort: None,
            thinking_keep: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            beta_api: None,
            tool_select: false,
            system_prompt_hash: "system-hash".into(),
            system_prompt: None,
            tools_hash: "tools-hash".into(),
            message_count: 3,
            turn_step: None,
            attempt: None,
            projection: None,
            dropped_count: None,
        }
    }

    #[test]
    fn model_and_minimal_request_wire_shape_match_source() {
        assert_eq!(LLM_REQUEST_TRACE_MODEL.name(), "llm.requestTrace");
        assert!(
            LLM_REQUEST_TRACE_MODEL
                .initial()
                .seen_tools_hashes
                .is_empty()
        );
        assert_eq!(LLM_TOOLS_SNAPSHOT.op_type(), "llm.tools_snapshot");
        assert_eq!(LLM_REQUEST.op_type(), "llm.request");

        let op = llm_request(request()).unwrap();
        assert_eq!(
            op.payload_value,
            serde_json::json!({
                "kind": "loop",
                "provider": "openai",
                "model": "kimi-k2",
                "toolSelect": false,
                "systemPromptHash": "system-hash",
                "toolsHash": "tools-hash",
                "messageCount": 3
            })
        );
    }

    #[test]
    fn full_request_preserves_every_external_field_name_and_enum_value() {
        let mut payload = request();
        payload.kind = LlmRequestKind::Compaction;
        payload.model_alias = Some("kimi".into());
        payload.thinking_effort = Some(ThinkingEffort::from("high"));
        payload.thinking_keep = Some("all".into());
        payload.temperature = Some(0.2);
        payload.top_p = Some(0.9);
        payload.max_tokens = Some(4096);
        payload.beta_api = Some(true);
        payload.tool_select = true;
        payload.system_prompt = Some("system".into());
        payload.turn_step = Some("7:2".into());
        payload.attempt = Some("strict".into());
        payload.projection = Some(LlmRequestProjection::MediaStripped);
        payload.dropped_count = Some(2);

        let value = llm_request(payload).unwrap().payload_value;
        assert_eq!(value["kind"], "compaction");
        assert_eq!(value["modelAlias"], "kimi");
        assert_eq!(value["thinkingEffort"], "high");
        assert_eq!(value["thinkingKeep"], "all");
        assert_eq!(value["topP"], 0.9);
        assert_eq!(value["maxTokens"], 4096.0);
        assert_eq!(value["betaApi"], true);
        assert_eq!(value["toolSelect"], true);
        assert_eq!(value["systemPrompt"], "system");
        assert_eq!(value["turnStep"], "7:2");
        assert_eq!(value["projection"], "media-stripped");
        assert_eq!(value["droppedCount"], 2.0);
    }

    #[test]
    fn tools_snapshot_records_unique_hashes_in_first_seen_order() {
        let first_payload = LlmToolsSnapshotPayload {
            hash: "a".into(),
            tools: vec![tool("Read")],
        };
        let state = apply_tools_snapshot(LlmRequestTraceState::default(), &first_payload);
        let duplicate = apply_tools_snapshot(
            state,
            &LlmToolsSnapshotPayload {
                hash: "a".into(),
                tools: vec![tool("Different")],
            },
        );
        let final_state = apply_tools_snapshot(
            duplicate,
            &LlmToolsSnapshotPayload {
                hash: "b".into(),
                tools: Vec::new(),
            },
        );
        assert_eq!(final_state.seen_tools_hashes, ["a", "b"]);
    }

    #[test]
    fn request_replay_validates_payload_and_leaves_trace_state_unchanged() {
        LazyLock::force(&LLM_REQUEST);
        let descriptor = registered_op("llm.request").unwrap();
        assert!(Op::from_wire(descriptor.clone(), serde_json::json!({"kind": "loop"})).is_err());

        let payload = serde_json::to_value(request()).unwrap();
        let replay = Op::from_wire(descriptor.clone(), payload).unwrap();
        let initial = LlmRequestTraceState {
            seen_tools_hashes: vec!["tools".into()],
        };
        let state = descriptor
            .apply(Box::new(initial.clone()), replay.payload())
            .unwrap()
            .downcast::<LlmRequestTraceState>()
            .unwrap();
        assert_eq!(*state, initial);
    }

    #[test]
    fn legacy_float_max_tokens_still_replays() {
        LazyLock::force(&LLM_REQUEST);
        let descriptor = registered_op("llm.request").unwrap();
        let mut payload = serde_json::to_value(request()).unwrap();
        payload["maxTokens"] = serde_json::json!(131072.0);
        let parsed: LlmRequestPayload = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(parsed.max_tokens, Some(131_072));
        assert!(Op::from_wire(descriptor, payload).is_ok());
    }

    #[test]
    fn tool_schema_requires_object_parameters_during_replay() {
        LazyLock::force(&LLM_TOOLS_SNAPSHOT);
        let descriptor = registered_op("llm.tools_snapshot").unwrap();
        let valid = serde_json::json!({
            "hash": "hash",
            "tools": [{"name": "Read", "description": "read", "parameters": {}}]
        });
        assert!(Op::from_wire(descriptor.clone(), valid).is_ok());
        let invalid = serde_json::json!({
            "hash": "hash",
            "tools": [{"name": "Read", "description": "read", "parameters": []}]
        });
        assert!(Op::from_wire(descriptor, invalid).is_err());
    }
}
