//! Durable user-tool registration wire operations.
//!
//! Original: `agent/userTool/userToolOps.ts`.

use std::{collections::HashMap, sync::LazyLock};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

use super::UserToolRegistration;

pub type UserToolModelState = HashMap<String, UserToolRegistration>;

pub static USER_TOOL_MODEL: LazyLock<ModelDef<UserToolModelState>> =
    LazyLock::new(|| define_model("userTool", HashMap::new, ModelOptions::default()));

pub static REGISTER_USER_TOOL: LazyLock<DefinedOp<UserToolModelState, UserToolRegistration>> =
    LazyLock::new(|| {
        USER_TOOL_MODEL
            .define_op(
                "tools.register_user_tool",
                DefineOpOptions::new(apply_register_user_tool),
            )
            .expect("tools.register_user_tool must have one global definition")
    });

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct UnregisterUserToolInput {
    pub name: String,
}

pub static UNREGISTER_USER_TOOL: LazyLock<DefinedOp<UserToolModelState, UnregisterUserToolInput>> =
    LazyLock::new(|| {
        USER_TOOL_MODEL
            .define_op(
                "tools.unregister_user_tool",
                DefineOpOptions::new(apply_unregister_user_tool),
            )
            .expect("tools.unregister_user_tool must have one global definition")
    });

// Original: registerUserTool.apply(). Rust compares serialized parameter
// contents because JavaScript's source comparison uses object identity.
pub fn apply_register_user_tool(
    state: UserToolModelState,
    registration: &UserToolRegistration,
) -> UserToolModelState {
    if state.get(&registration.name) == Some(registration) {
        return state;
    }
    let mut next = state;
    next.insert(registration.name.clone(), registration.clone());
    next
}

// Original: unregisterUserTool.apply().
pub fn apply_unregister_user_tool(
    state: UserToolModelState,
    input: &UnregisterUserToolInput,
) -> UserToolModelState {
    if !state.contains_key(&input.name) {
        return state;
    }
    let mut next = state;
    next.remove(&input.name);
    next
}

pub fn register_user_tool(registration: UserToolRegistration) -> Result<Op, serde_json::Error> {
    REGISTER_USER_TOOL.create(registration)
}

pub fn unregister_user_tool(name: impl Into<String>) -> Result<Op, serde_json::Error> {
    UNREGISTER_USER_TOOL.create(UnregisterUserToolInput { name: name.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(name: &str, description: &str) -> UserToolRegistration {
        UserToolRegistration {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::Map::new(),
        }
    }

    #[test]
    fn operations_preserve_noop_identity_and_wire_names() {
        let first = registration("host", "first");
        let state = apply_register_user_tool(HashMap::new(), &first);
        let same = apply_register_user_tool(state.clone(), &first);
        assert_eq!(same, state);
        let replaced = apply_register_user_tool(state, &registration("host", "second"));
        assert_eq!(replaced["host"].description, "second");
        let removed = apply_unregister_user_tool(
            replaced,
            &UnregisterUserToolInput {
                name: "host".into(),
            },
        );
        assert!(removed.is_empty());
        assert_eq!(REGISTER_USER_TOOL.op_type(), "tools.register_user_tool");
        assert_eq!(UNREGISTER_USER_TOOL.op_type(), "tools.unregister_user_tool");
    }
}
