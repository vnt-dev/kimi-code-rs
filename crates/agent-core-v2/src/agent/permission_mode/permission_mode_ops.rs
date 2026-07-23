use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::{
    agent::permission_policy::PermissionMode,
    wire::{
        model::{ModelCrossReducer, ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetPermissionModePayload {
    pub mode: PermissionMode,
}

// Original:
//   packages/agent-core-v2/src/agent/permissionMode/permissionModeOps.ts
//   PermissionModeModel / PermissionModeConfiguredModel
pub static PERMISSION_MODE_MODEL: LazyLock<ModelDef<PermissionMode>> = LazyLock::new(|| {
    define_model(
        "permissionMode",
        || PermissionMode::Manual,
        ModelOptions::default(),
    )
});

pub static PERMISSION_MODE_CONFIGURED_MODEL: LazyLock<ModelDef<bool>> = LazyLock::new(|| {
    define_model(
        "permissionMode.configured",
        || false,
        ModelOptions {
            reducers: vec![ModelCrossReducer::typed(
                "permission.set_mode",
                |_state, _payload: &SetPermissionModePayload| true,
            )],
            ..ModelOptions::default()
        },
    )
});

pub static SET_PERMISSION_MODE: LazyLock<DefinedOp<PermissionMode, SetPermissionModePayload>> =
    LazyLock::new(|| {
        PERMISSION_MODE_MODEL
            .define_op(
                "permission.set_mode",
                DefineOpOptions::new(|_state, payload: &SetPermissionModePayload| payload.mode),
            )
            .expect("permission.set_mode must have one global definition")
    });

// Original: permissionModeOps.ts, setMode().
pub fn set_permission_mode(mode: PermissionMode) -> Result<Op, serde_json::Error> {
    SET_PERMISSION_MODE.create(SetPermissionModePayload { mode })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{model::model_cross_reducers, op::ErasedOpDescriptor};

    #[test]
    fn models_preserve_names_and_initial_states() {
        assert_eq!(PERMISSION_MODE_MODEL.name(), "permissionMode");
        assert_eq!(PERMISSION_MODE_MODEL.initial(), PermissionMode::Manual);
        assert_eq!(
            PERMISSION_MODE_CONFIGURED_MODEL.name(),
            "permissionMode.configured"
        );
        assert!(!PERMISSION_MODE_CONFIGURED_MODEL.initial());
        assert_eq!(SET_PERMISSION_MODE.descriptor().persist(), None);
    }

    #[test]
    fn op_replaces_mode_and_serializes_a_flat_payload() {
        let op = set_permission_mode(PermissionMode::Auto).unwrap();
        assert_eq!(op.op_type, "permission.set_mode");
        assert_eq!(op.payload_value, serde_json::json!({"mode": "auto"}));
        let next = SET_PERMISSION_MODE
            .descriptor()
            .apply(Box::new(PermissionMode::Manual), op.payload())
            .unwrap()
            .downcast::<PermissionMode>()
            .unwrap();
        assert_eq!(*next, PermissionMode::Auto);
    }

    #[test]
    fn configured_cross_reducer_marks_even_explicit_manual() {
        std::sync::LazyLock::force(&PERMISSION_MODE_CONFIGURED_MODEL);
        let op = set_permission_mode(PermissionMode::Manual).unwrap();
        let reducers = model_cross_reducers("permission.set_mode");
        let configured = reducers
            .iter()
            .find(|entry| entry.model.name() == "permissionMode.configured")
            .unwrap()
            .apply(Box::new(false), op.payload())
            .unwrap()
            .downcast::<bool>()
            .unwrap();
        assert!(*configured);
    }

    #[test]
    fn permission_mode_wire_values_round_trip() {
        for (mode, value) in [
            (PermissionMode::Manual, "manual"),
            (PermissionMode::Auto, "auto"),
            (PermissionMode::Yolo, "yolo"),
        ] {
            assert_eq!(serde_json::to_value(mode).unwrap(), value);
            assert_eq!(
                serde_json::from_value::<PermissionMode>(serde_json::json!(value)).unwrap(),
                mode
            );
        }
    }
}
