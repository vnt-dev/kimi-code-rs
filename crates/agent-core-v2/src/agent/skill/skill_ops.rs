use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};

use crate::{
    agent::context_memory::SkillActivationOrigin,
    wire::{
        model::{ModelDef, ModelOptions, define_model},
        op::{DefineOpOptions, DefinedOp, Op},
    },
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillActivatePayload {
    pub origin: SkillActivationOrigin,
}

pub static SKILL_MODEL: LazyLock<ModelDef<()>> =
    LazyLock::new(|| define_model("skill", || (), ModelOptions::default()));

pub static SKILL_ACTIVATE: LazyLock<DefinedOp<(), SkillActivatePayload>> = LazyLock::new(|| {
    let mut options = DefineOpOptions::new(|state, _payload: &SkillActivatePayload| state);
    options.persist = Some(false);
    options.to_event = Some(Arc::new(|payload, _state| {
        let origin = &payload.origin;
        let mut event = serde_json::json!({
            "type": "skill.activated",
            "activationId": origin.activation_id,
            "skillName": origin.skill_name,
            "trigger": origin.trigger,
        })
        .as_object()
        .cloned()
        .expect("skill event literal is an object");
        if let Some(skill_args) = &origin.skill_args {
            event.insert(
                "skillArgs".into(),
                serde_json::Value::String(skill_args.clone()),
            );
        }
        if let Some(skill_path) = &origin.skill_path {
            event.insert(
                "skillPath".into(),
                serde_json::Value::String(skill_path.clone()),
            );
        }
        if let Some(skill_source) = origin.skill_source {
            event.insert(
                "skillSource".into(),
                serde_json::to_value(skill_source)
                    .expect("SkillSource is always JSON serializable"),
            );
        }
        Some(serde_json::Value::Object(event))
    }));
    SKILL_MODEL
        .define_op("skill.activate", options)
        .expect("skill.activate must have one global definition")
});

// Original: skillOps.ts, skillActivate(). The activation id is supplied by
// the caller, keeping this transient identity reducer deterministic.
pub fn skill_activate(origin: SkillActivationOrigin) -> Result<Op, serde_json::Error> {
    SKILL_ACTIVATE.create(SkillActivatePayload { origin })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::context_memory::{SkillActivationOriginKind, SkillActivationTrigger, SkillSource},
        wire::op::ErasedOpDescriptor,
    };

    fn origin() -> SkillActivationOrigin {
        SkillActivationOrigin {
            kind: SkillActivationOriginKind::SkillActivation,
            activation_id: "activation-1".into(),
            skill_name: "review".into(),
            skill_args: Some("--strict".into()),
            trigger: SkillActivationTrigger::NestedSkill,
            skill_type: Some("flow".into()),
            skill_path: Some("/skills/review/SKILL.md".into()),
            skill_source: Some(SkillSource::Project),
        }
    }

    #[test]
    fn model_and_transient_payload_match_source() {
        assert_eq!(SKILL_MODEL.name(), "skill");
        assert_eq!(SKILL_MODEL.initial(), ());
        assert_eq!(SKILL_ACTIVATE.op_type(), "skill.activate");
        assert_eq!(SKILL_ACTIVATE.descriptor().persist_value(), Some(false));
        assert_eq!(
            skill_activate(origin()).unwrap().payload_value,
            serde_json::json!({
                "origin": {
                    "kind": "skill_activation",
                    "activationId": "activation-1",
                    "skillName": "review",
                    "skillArgs": "--strict",
                    "trigger": "nested-skill",
                    "skillType": "flow",
                    "skillPath": "/skills/review/SKILL.md",
                    "skillSource": "project"
                }
            })
        );
    }

    #[test]
    fn op_is_identity_and_projects_activation_without_internal_fields() {
        let op = skill_activate(origin()).unwrap();
        let state = SKILL_ACTIVATE
            .descriptor()
            .apply(Box::new(()), op.payload())
            .unwrap();
        state.downcast::<()>().unwrap();
        assert_eq!(
            SKILL_ACTIVATE
                .descriptor()
                .to_event(op.payload(), &())
                .unwrap(),
            Some(serde_json::json!({
                "type": "skill.activated",
                "activationId": "activation-1",
                "skillName": "review",
                "trigger": "nested-skill",
                "skillArgs": "--strict",
                "skillPath": "/skills/review/SKILL.md",
                "skillSource": "project"
            }))
        );
    }

    #[test]
    fn event_omits_optional_fields_like_the_source_object() {
        let mut minimal = origin();
        minimal.skill_args = None;
        minimal.skill_path = None;
        minimal.skill_source = None;
        let op = skill_activate(minimal).unwrap();
        assert_eq!(
            SKILL_ACTIVATE
                .descriptor()
                .to_event(op.payload(), &())
                .unwrap(),
            Some(serde_json::json!({
                "type": "skill.activated",
                "activationId": "activation-1",
                "skillName": "review",
                "trigger": "nested-skill"
            }))
        );
    }
}
