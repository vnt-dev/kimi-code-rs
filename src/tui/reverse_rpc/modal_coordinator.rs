use std::{collections::VecDeque, sync::Arc};

use super::types::{ApprovalPanelData, QuestionPanelData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverseRpcModalOwner {
    Approval,
    Question,
}

pub trait ReverseRpcModalUiHooks: Send + Sync {
    fn show_approval_panel(&self, payload: &ApprovalPanelData);
    fn hide_approval_panel(&self);
    fn show_question_dialog(&self, payload: &QuestionPanelData);
    fn hide_question_dialog(&self);
}

enum ReverseRpcModalEntry {
    Approval(ApprovalPanelData),
    Question(QuestionPanelData),
}

impl ReverseRpcModalEntry {
    fn owner(&self) -> ReverseRpcModalOwner {
        match self {
            Self::Approval(_) => ReverseRpcModalOwner::Approval,
            Self::Question(_) => ReverseRpcModalOwner::Question,
        }
    }
}

/// Serializes approval and question modals onto one TUI surface.
///
/// Original:
///   apps/kimi-code/src/tui/reverse-rpc/modal-coordinator.ts
///   ReverseRpcModalCoordinator
pub struct ReverseRpcModalCoordinator {
    hooks: Arc<dyn ReverseRpcModalUiHooks>,
    active: Option<ReverseRpcModalEntry>,
    queued: VecDeque<ReverseRpcModalEntry>,
}

impl ReverseRpcModalCoordinator {
    pub fn new(hooks: Arc<dyn ReverseRpcModalUiHooks>) -> Self {
        Self {
            hooks,
            active: None,
            queued: VecDeque::new(),
        }
    }

    pub fn show_approval(&mut self, payload: ApprovalPanelData) {
        self.show(ReverseRpcModalEntry::Approval(payload));
    }

    pub fn show_question(&mut self, payload: QuestionPanelData) {
        self.show(ReverseRpcModalEntry::Question(payload));
    }

    pub fn hide(&mut self, owner: ReverseRpcModalOwner) {
        if self
            .active
            .as_ref()
            .is_some_and(|entry| entry.owner() == owner)
        {
            let active = self.active.take().expect("active owner was checked");
            self.hide_entry(&active);
            self.show_next();
            return;
        }
        if let Some(index) = self.queued.iter().position(|entry| entry.owner() == owner) {
            self.queued.remove(index);
        }
    }

    pub fn clear(&mut self) {
        let active = self.active.take();
        self.queued.clear();
        if let Some(active) = active {
            self.hide_entry(&active);
        }
    }

    fn show(&mut self, entry: ReverseRpcModalEntry) {
        let Some(active) = &self.active else {
            self.show_entry(&entry);
            self.active = Some(entry);
            return;
        };
        if active.owner() == entry.owner() {
            self.show_entry(&entry);
            self.active = Some(entry);
            return;
        }
        if let Some(index) = self
            .queued
            .iter()
            .position(|queued| queued.owner() == entry.owner())
        {
            self.queued[index] = entry;
        } else {
            self.queued.push_back(entry);
        }
    }

    fn show_next(&mut self) {
        let Some(next) = self.queued.pop_front() else {
            return;
        };
        self.show_entry(&next);
        self.active = Some(next);
    }

    fn show_entry(&self, entry: &ReverseRpcModalEntry) {
        match entry {
            ReverseRpcModalEntry::Approval(payload) => self.hooks.show_approval_panel(payload),
            ReverseRpcModalEntry::Question(payload) => self.hooks.show_question_dialog(payload),
        }
    }

    fn hide_entry(&self, entry: &ReverseRpcModalEntry) {
        match entry {
            ReverseRpcModalEntry::Approval(_) => self.hooks.hide_approval_panel(),
            ReverseRpcModalEntry::Question(_) => self.hooks.hide_question_dialog(),
        }
    }
}
