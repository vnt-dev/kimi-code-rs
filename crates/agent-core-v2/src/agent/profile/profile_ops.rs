//! Replayable agent-profile configuration models and operations.
//!
//! Original: `packages/agent-core-v2/src/agent/profile/profileOps.ts`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    kosong::contract::provider::ThinkingEffort,
    wire::{
        model::{ModelCrossReducer, ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

use super::{ProfileError, ProfileErrorCode, create_profile_error};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileModelState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub thinking_level: String,
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<String>>,
}

impl Default for ProfileModelState {
    fn default() -> Self {
        Self {
            cwd: None,
            model_alias: None,
            profile_name: None,
            thinking_level: "off".into(),
            system_prompt: String::new(),
            disallowed_tools: None,
            subagents: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBindPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub thinking_effort: ThinkingEffort,
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool_names: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdatePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<ThinkingEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
}

pub type ActiveToolsState = Option<Vec<String>>;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyProfilePayload {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetActiveToolsPayload {
    pub names: Vec<String>,
}

pub static PROFILE_MODEL: LazyLock<ModelDef<ProfileModelState>> = LazyLock::new(|| {
    define_model(
        "profile",
        ProfileModelState::default,
        ModelOptions::default(),
    )
});

pub static ACTIVE_TOOLS_MODEL: LazyLock<ModelDef<ActiveToolsState>> = LazyLock::new(|| {
    define_model(
        "profile.activeTools",
        || None,
        ModelOptions {
            blobs: None,
            reducers: vec![ModelCrossReducer::typed(
                "profile.bind",
                apply_bound_active_tools,
            )],
        },
    )
});

pub static PROFILE_BIND: LazyLock<DefinedOp<ProfileModelState, ProfileBindPayload>> =
    LazyLock::new(|| {
        PROFILE_MODEL
            .define_op("profile.bind", DefineOpOptions::new(apply_profile_bind))
            .expect("profile.bind must have one global definition")
    });

pub static CONFIG_UPDATE: LazyLock<DefinedOp<ProfileModelState, ConfigUpdatePayload>> =
    LazyLock::new(|| {
        PROFILE_MODEL
            .define_op(
                "config.update",
                DefineOpOptions::new(apply_config_update).with_apply_validation(
                    |_state, payload| {
                        config_update_thinking_level(payload)
                            .map(|_| ())
                            .map_err(|error| error as _)
                    },
                ),
            )
            .expect("config.update must have one global definition")
    });

pub static SET_ACTIVE_TOOLS: LazyLock<DefinedOp<ActiveToolsState, SetActiveToolsPayload>> =
    LazyLock::new(|| {
        ACTIVE_TOOLS_MODEL
            .define_op(
                "tools.set_active_tools",
                DefineOpOptions::new(apply_set_active_tools),
            )
            .expect("tools.set_active_tools must have one global definition")
    });

pub static RESET_ACTIVE_TOOLS: LazyLock<DefinedOp<ActiveToolsState, EmptyProfilePayload>> =
    LazyLock::new(|| {
        ACTIVE_TOOLS_MODEL
            .define_op(
                "tools.reset_active_tools",
                DefineOpOptions::new(apply_reset_active_tools),
            )
            .expect("tools.reset_active_tools must have one global definition")
    });

fn apply_profile_bind(state: ProfileModelState, payload: &ProfileBindPayload) -> ProfileModelState {
    ProfileModelState {
        cwd: payload.cwd.clone().or(state.cwd),
        model_alias: payload.model_alias.clone().or(state.model_alias),
        profile_name: payload.profile_name.clone().or(state.profile_name),
        thinking_level: payload.thinking_effort.as_str().to_owned(),
        system_prompt: payload.system_prompt.clone(),
        disallowed_tools: Some(payload.disallowed_tools.clone()),
        subagents: payload.subagents.clone(),
    }
}

fn apply_config_update(
    state: ProfileModelState,
    payload: &ConfigUpdatePayload,
) -> ProfileModelState {
    let Ok(thinking_level) = config_update_thinking_level(payload) else {
        // Dispatch and replay invoke CONFIG_UPDATE's validator before this
        // reducer; preserve state for a private direct invocation.
        return state;
    };
    let mut next = state.clone();
    let mut changed = false;
    update_option_if_changed(&mut next.cwd, &payload.cwd, &mut changed);
    update_option_if_changed(&mut next.model_alias, &payload.model_alias, &mut changed);
    update_option_if_changed(&mut next.profile_name, &payload.profile_name, &mut changed);
    if let Some(thinking_level) = thinking_level
        && next.thinking_level != thinking_level.as_str()
    {
        next.thinking_level = thinking_level.as_str().to_owned();
        changed = true;
    }
    update_value_if_changed(
        &mut next.system_prompt,
        &payload.system_prompt,
        &mut changed,
    );
    if let Some(disallowed_tools) = &payload.disallowed_tools
        && next.disallowed_tools.as_ref() != Some(disallowed_tools)
    {
        next.disallowed_tools = Some(disallowed_tools.clone());
        changed = true;
    }
    if changed { next } else { state }
}

fn update_option_if_changed<T: Clone + PartialEq>(
    target: &mut Option<T>,
    update: &Option<T>,
    changed: &mut bool,
) {
    if let Some(update) = update
        && target.as_ref() != Some(update)
    {
        *target = Some(update.clone());
        *changed = true;
    }
}

fn update_value_if_changed<T: Clone + PartialEq>(
    target: &mut T,
    update: &Option<T>,
    changed: &mut bool,
) {
    if let Some(update) = update
        && target != update
    {
        *target = update.clone();
        *changed = true;
    }
}

pub fn config_update_thinking_level(
    payload: &ConfigUpdatePayload,
) -> Result<Option<ThinkingEffort>, Box<ProfileError>> {
    match (&payload.thinking_effort, &payload.thinking_level) {
        (Some(effort), Some(level)) if effort != level => Err(Box::new(create_profile_error(
            ProfileErrorCode::ThinkingAliasConflict,
            format!(
                "config.update has conflicting thinkingEffort ({effort}) and legacy thinkingLevel ({level})"
            ),
            Some(Map::from_iter([
                ("type".into(), Value::String("config.update".into())),
                (
                    "thinkingEffort".into(),
                    Value::String(effort.as_str().into()),
                ),
                ("thinkingLevel".into(), Value::String(level.as_str().into())),
            ])),
        ))),
        (Some(effort), _) => Ok(Some(effort.clone())),
        (None, Some(level)) => Ok(Some(level.clone())),
        (None, None) => Ok(None),
    }
}

fn apply_bound_active_tools(_: ActiveToolsState, payload: &ProfileBindPayload) -> ActiveToolsState {
    payload.active_tool_names.clone()
}

fn apply_set_active_tools(
    state: ActiveToolsState,
    payload: &SetActiveToolsPayload,
) -> ActiveToolsState {
    if state.as_ref() == Some(&payload.names) {
        state
    } else {
        Some(payload.names.clone())
    }
}

fn apply_reset_active_tools(_: ActiveToolsState, _: &EmptyProfilePayload) -> ActiveToolsState {
    None
}

pub fn profile_bind(payload: ProfileBindPayload) -> Result<Op, serde_json::Error> {
    PROFILE_BIND.create(payload)
}

pub fn config_update(payload: ConfigUpdatePayload) -> Result<Op, serde_json::Error> {
    CONFIG_UPDATE.create(payload)
}

pub fn set_active_tools(names: Vec<String>) -> Result<Op, serde_json::Error> {
    SET_ACTIVE_TOOLS.create(SetActiveToolsPayload { names })
}

pub fn reset_active_tools() -> Result<Op, serde_json::Error> {
    RESET_ACTIVE_TOOLS.create(EmptyProfilePayload {})
}

#[cfg(test)]
mod tests {
    use crate::wire::model::model_cross_reducers;

    use super::*;

    #[test]
    fn models_and_wire_payloads_match_source_contract() {
        assert_eq!(PROFILE_MODEL.name(), "profile");
        assert_eq!(PROFILE_MODEL.initial(), ProfileModelState::default());
        assert_eq!(ACTIVE_TOOLS_MODEL.name(), "profile.activeTools");
        assert_eq!(PROFILE_BIND.op_type(), "profile.bind");
        assert_eq!(CONFIG_UPDATE.op_type(), "config.update");
        assert_eq!(
            config_update(ConfigUpdatePayload {
                thinking_effort: Some("high".into()),
                ..ConfigUpdatePayload::default()
            })
            .unwrap()
            .payload_value,
            serde_json::json!({"thinkingEffort": "high"})
        );
    }

    #[test]
    fn config_update_preserves_legacy_effort_and_rejects_conflicts() {
        let legacy = ConfigUpdatePayload {
            thinking_level: Some("low".into()),
            ..ConfigUpdatePayload::default()
        };
        assert_eq!(
            config_update_thinking_level(&legacy).unwrap(),
            Some(ThinkingEffort::from("low"))
        );
        let conflict = ConfigUpdatePayload {
            thinking_effort: Some("high".into()),
            thinking_level: Some("low".into()),
            ..ConfigUpdatePayload::default()
        };
        let error = config_update_thinking_level(&conflict).unwrap_err();
        assert_eq!(error.code, "profile.thinking_alias_conflict");
        assert_eq!(error.name, "ProfileError");
        assert!(error.message.contains("thinkingEffort (high)"));
    }

    #[test]
    fn active_tools_reset_and_profile_bind_cross_reducer_match_source() {
        LazyLock::force(&ACTIVE_TOOLS_MODEL);
        assert_eq!(
            apply_set_active_tools(
                None,
                &SetActiveToolsPayload {
                    names: vec!["Read".into()]
                }
            ),
            Some(vec!["Read".into()])
        );
        assert_eq!(
            apply_reset_active_tools(Some(vec!["Read".into()]), &EmptyProfilePayload {}),
            None
        );
        let reducers = model_cross_reducers("profile.bind");
        let reducer = reducers
            .iter()
            .find(|reducer| reducer.model.id() == ACTIVE_TOOLS_MODEL.id())
            .unwrap();
        let payload = ProfileBindPayload {
            cwd: None,
            model_alias: None,
            profile_name: None,
            thinking_effort: "off".into(),
            system_prompt: String::new(),
            active_tool_names: Some(vec!["Write".into()]),
            disallowed_tools: vec![],
            subagents: None,
        };
        let state = reducer
            .apply(Box::new(None::<Vec<String>>), &payload)
            .unwrap()
            .downcast::<ActiveToolsState>()
            .unwrap();
        assert_eq!(*state, Some(vec!["Write".into()]));
    }
}
