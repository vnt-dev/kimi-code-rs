//! Session-level root store.
//!
//! Original:
//!   `packages/transcript/src/store/transcriptStore.ts`

use indexmap::{IndexMap, map::Entry};
use serde::{Deserialize, Serialize};

use crate::model::AgentId;

use super::{AgentTranscript, Disposable, ListenerRegistry};

pub type RosterListener = dyn FnMut(&[AgentDescriptor]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    Main,
    Sub,
    Independent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDescriptor {
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<AgentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposed_at: Option<String>,
}

pub struct TranscriptStore {
    pub session_id: String,
    agents: IndexMap<AgentId, AgentTranscript>,
    descriptors: IndexMap<AgentId, AgentDescriptor>,
    roster_listeners: ListenerRegistry<Vec<AgentDescriptor>>,
}

impl TranscriptStore {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            agents: IndexMap::new(),
            descriptors: IndexMap::new(),
            roster_listeners: ListenerRegistry::new(),
        }
    }

    /// Lazily create or fetch one agent transcript.
    pub fn ensure_agent(
        &mut self,
        agent_id: AgentId,
        descriptor: Option<AgentDescriptor>,
    ) -> &mut AgentTranscript {
        if !self.agents.contains_key(&agent_id) {
            self.agents
                .insert(agent_id.clone(), AgentTranscript::new(agent_id.clone()));
        }
        if let Some(descriptor) = descriptor
            && self.descriptors.get(&agent_id) != Some(&descriptor)
        {
            self.descriptors.insert(agent_id.clone(), descriptor);
            self.emit_roster();
        }
        match self.agents.entry(agent_id.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(AgentTranscript::new(agent_id)),
        }
    }

    pub fn get_agent(&self, agent_id: &AgentId) -> Option<&AgentTranscript> {
        self.agents.get(agent_id)
    }

    pub fn get_agent_mut(&mut self, agent_id: &AgentId) -> Option<&mut AgentTranscript> {
        self.agents.get_mut(agent_id)
    }

    /// Drop an agent transcript and its roster descriptor.
    pub fn remove_agent(&mut self, agent_id: &AgentId) -> bool {
        let removed = self.agents.shift_remove(agent_id).is_some();
        let descriptor_removed = self.descriptors.shift_remove(agent_id).is_some();
        if descriptor_removed || removed {
            self.emit_roster();
        }
        removed
    }

    /// Replace one roster descriptor when its value changed.
    pub fn describe_agent(&mut self, descriptor: AgentDescriptor) {
        let agent_id = descriptor.agent_id.clone();
        if self.descriptors.get(&agent_id) != Some(&descriptor) {
            self.descriptors.insert(agent_id, descriptor);
            self.emit_roster();
        }
    }

    pub fn mark_disposed(&mut self, agent_id: &AgentId, disposed_at: impl Into<String>) {
        let Some(descriptor) = self.descriptors.get(agent_id) else {
            return;
        };
        if descriptor.disposed_at.is_some() {
            return;
        }
        let mut descriptor = descriptor.clone();
        descriptor.disposed_at = Some(disposed_at.into());
        self.describe_agent(descriptor);
    }

    pub fn agents(&self) -> Vec<AgentDescriptor> {
        self.descriptors.values().cloned().collect()
    }

    pub fn on_roster_change(
        &self,
        mut listener: impl FnMut(&[AgentDescriptor]) + 'static,
    ) -> Disposable {
        self.roster_listeners
            .register(move |agents: &Vec<AgentDescriptor>| listener(agents))
    }

    fn emit_roster(&self) {
        self.roster_listeners.emit(&self.agents());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn descriptor(id: &str, r#type: AgentType) -> AgentDescriptor {
        AgentDescriptor {
            agent_id: AgentId::from(id),
            r#type: Some(r#type),
            parent_agent_id: None,
            label: None,
            created_at: None,
            disposed_at: None,
        }
    }

    #[test]
    fn lazily_tracks_roster_removal_and_first_disposal_stamp() {
        let mut store = TranscriptStore::new("session-1");
        store.ensure_agent(
            AgentId::from("main"),
            Some(descriptor("main", AgentType::Main)),
        );
        let rosters = Rc::new(RefCell::new(Vec::new()));
        let callback_rosters = rosters.clone();
        let _listener = store.on_roster_change(move |agents| {
            callback_rosters.borrow_mut().push(
                agents
                    .iter()
                    .map(|agent| agent.agent_id.clone())
                    .collect::<Vec<_>>(),
            );
        });

        let mut sub = descriptor("sub-1", AgentType::Sub);
        sub.parent_agent_id = Some(AgentId::from("main"));
        store.ensure_agent(AgentId::from("sub-1"), Some(sub));
        assert!(store.remove_agent(&AgentId::from("sub-1")));
        assert_eq!(
            rosters.borrow().iter().map(Vec::len).collect::<Vec<_>>(),
            [2, 1]
        );

        store.mark_disposed(&AgentId::from("ghost"), "ignored");
        store.mark_disposed(&AgentId::from("main"), "first");
        store.mark_disposed(&AgentId::from("main"), "second");
        assert_eq!(store.agents()[0].disposed_at.as_deref(), Some("first"));
        assert_eq!(rosters.borrow().len(), 3);
    }
}
