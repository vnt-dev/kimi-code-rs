//! Persisted subagent relationship-label helpers.
//!
//! Original: `session/agentLifecycle/subagentMetadata.ts`.

use std::collections::BTreeMap;

use crate::session::session_metadata::{AgentMeta, AgentMetaType};

pub fn subagent_labels(
    parent_agent_id: &str,
    swarm_item: Option<&str>,
) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::from([("parentAgentId".into(), parent_agent_id.into())]);
    if let Some(swarm_item) = swarm_item {
        labels.insert("swarmItem".into(), swarm_item.into());
    }
    labels
}
pub fn labels_from_agent_meta(meta: &AgentMeta) -> Option<BTreeMap<String, String>> {
    let mut labels = meta.labels.clone().unwrap_or_default();
    if let Some(parent) = subagent_parent_agent_id(Some(meta)) {
        labels.insert("parentAgentId".into(), parent);
    }
    if let Some(item) = subagent_swarm_item(Some(meta)) {
        labels.insert("swarmItem".into(), item);
    }
    (!labels.is_empty()).then_some(labels)
}
pub fn is_subagent_meta(meta: Option<&AgentMeta>) -> bool {
    meta.is_some_and(|meta| {
        subagent_parent_agent_id(Some(meta)).is_some() || meta.r#type == Some(AgentMetaType::Sub)
    })
}
pub fn subagent_parent_agent_id(meta: Option<&AgentMeta>) -> Option<String> {
    meta.and_then(|meta| {
        first_non_empty([
            meta.labels
                .as_ref()
                .and_then(|labels| labels.get("parentAgentId").cloned()),
            meta.parent_agent_id.clone(),
        ])
    })
}
pub fn subagent_swarm_item(meta: Option<&AgentMeta>) -> Option<String> {
    meta.and_then(|meta| {
        first_non_empty([
            meta.labels
                .as_ref()
                .and_then(|labels| labels.get("swarmItem").cloned()),
            meta.swarm_item.clone(),
        ])
    })
}
fn first_non_empty(values: [Option<String>; 2]) -> Option<String> {
    values.into_iter().flatten().find(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_preserve_legacy_and_new_subagent_metadata() {
        let meta = AgentMeta {
            parent_agent_id: Some("legacy".into()),
            swarm_item: Some("item".into()),
            ..Default::default()
        };
        assert_eq!(
            subagent_parent_agent_id(Some(&meta)).as_deref(),
            Some("legacy")
        );
        assert!(is_subagent_meta(Some(&meta)));
        let labels = labels_from_agent_meta(&meta).unwrap();
        assert_eq!(labels["parentAgentId"], "legacy");
        assert_eq!(labels["swarmItem"], "item");
    }
}
