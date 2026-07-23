use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};

use crate::wire::{
    model::{ModelDef, ModelOptions, define_model},
    op::{DefineOpOptions, DefinedOp, Op},
};

// Original: packages/agent-core-v2/src/agent/swarm/swarm.ts, SwarmModeTrigger.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SwarmModeTrigger {
    Manual,
    Task,
    Tool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SwarmEnterPayload {
    pub trigger: SwarmModeTrigger,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SwarmExitPayload {}

pub static SWARM_MODEL: LazyLock<ModelDef<Option<SwarmModeTrigger>>> =
    LazyLock::new(|| define_model("swarm", || None, ModelOptions::default()));

pub static SWARM_ENTER: LazyLock<DefinedOp<Option<SwarmModeTrigger>, SwarmEnterPayload>> =
    LazyLock::new(|| {
        let mut options =
            DefineOpOptions::new(|_state, payload: &SwarmEnterPayload| Some(payload.trigger));
        options.to_event = Some(Arc::new(|_payload, _state| {
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "swarmMode": true,
            }))
        }));
        SWARM_MODEL
            .define_op("swarm_mode.enter", options)
            .expect("swarm_mode.enter must have one global definition")
    });

pub static SWARM_EXIT: LazyLock<DefinedOp<Option<SwarmModeTrigger>, SwarmExitPayload>> =
    LazyLock::new(|| {
        let mut options = DefineOpOptions::new(|_state, _payload: &SwarmExitPayload| None);
        options.to_event = Some(Arc::new(|_payload, _state| {
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "swarmMode": false,
            }))
        }));
        SWARM_MODEL
            .define_op("swarm_mode.exit", options)
            .expect("swarm_mode.exit must have one global definition")
    });

// Original: swarmOps.ts, swarmEnter()/swarmExit() Op creators.
pub fn swarm_enter(trigger: SwarmModeTrigger) -> Result<Op, serde_json::Error> {
    SWARM_ENTER.create(SwarmEnterPayload { trigger })
}

pub fn swarm_exit() -> Result<Op, serde_json::Error> {
    SWARM_EXIT.create(SwarmExitPayload {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::op::ErasedOpDescriptor;

    #[test]
    fn model_and_ops_preserve_state_and_wire_payloads() {
        assert_eq!(SWARM_MODEL.name(), "swarm");
        assert_eq!(SWARM_MODEL.initial(), None);
        let enter = swarm_enter(SwarmModeTrigger::Tool).unwrap();
        assert_eq!(
            enter.payload_value,
            serde_json::json!({ "trigger": "tool" })
        );
        assert_eq!(swarm_exit().unwrap().payload_value, serde_json::json!({}));
    }

    #[test]
    fn ops_project_status_events() {
        let enter = swarm_enter(SwarmModeTrigger::Task).unwrap();
        assert_eq!(
            SWARM_ENTER
                .descriptor()
                .to_event(enter.payload(), &Some(SwarmModeTrigger::Task))
                .unwrap(),
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "swarmMode": true
            }))
        );
        let exit = swarm_exit().unwrap();
        assert_eq!(
            SWARM_EXIT
                .descriptor()
                .to_event(exit.payload(), &None::<SwarmModeTrigger>)
                .unwrap(),
            Some(serde_json::json!({
                "type": "agent.status.updated",
                "swarmMode": false
            }))
        );
    }
}
