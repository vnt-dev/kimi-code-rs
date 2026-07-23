//! Turn-owned FIFO queue and merge batching for loop step requests.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/stepRequestQueue.ts`.

use std::sync::Arc;

use super::StepRequest;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StepRequestQueuePosition {
    Head,
    #[default]
    Tail,
}

pub struct StepRequestBatch {
    pub driver: Arc<dyn StepRequest>,
    pub merged: Vec<Arc<dyn StepRequest>>,
}

#[derive(Default)]
pub struct StepRequestQueue {
    items: Vec<Arc<dyn StepRequest>>,
}

impl StepRequestQueue {
    pub fn new() -> Self {
        Self::default()
    }

    // Original: StepRequestQueue.enqueue().
    pub fn enqueue(&mut self, request: Arc<dyn StepRequest>, at: StepRequestQueuePosition) {
        match at {
            StepRequestQueuePosition::Head => self.items.insert(0, request),
            StepRequestQueuePosition::Tail => self.items.push(request),
        }
    }

    // Original: StepRequestQueue.hasPendingRequests().
    pub fn has_pending_requests(&self) -> bool {
        self.items.iter().any(|item| !item.aborted())
    }

    // Original: StepRequestQueue.takeNextBatch().
    pub fn take_next_batch(&mut self) -> Option<StepRequestBatch> {
        self.discard_aborted();
        if self.items.is_empty() {
            return None;
        }
        let driver_index = self
            .items
            .iter()
            .position(|item| !item.mergeable())
            .unwrap_or(0);
        let driver = Arc::clone(&self.items[driver_index]);
        let mut merged = Vec::new();
        let mut rest = Vec::new();
        for (index, item) in self.items.drain(..).enumerate() {
            if index == driver_index {
                continue;
            }
            if item.mergeable() {
                merged.push(item);
            } else {
                rest.push(item);
            }
        }
        self.items = rest;
        Some(StepRequestBatch { driver, merged })
    }

    // Original: StepRequestQueue.drain(). Aborted entries are included when
    // callers explicitly drain rather than request a materialization batch.
    pub fn drain(&mut self) -> Vec<Arc<dyn StepRequest>> {
        self.items.drain(..).collect()
    }

    // Original: StepRequestQueue.abortTurnScoped().
    pub fn abort_turn_scoped(&mut self) {
        for item in &self.items {
            if item.turn_scoped() {
                item.abort();
            }
        }
        self.discard_aborted();
    }

    fn discard_aborted(&mut self) {
        self.items.retain(|item| !item.aborted());
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::loop_::{
        ContinuationStepRequest, MessageStepRequestOptions, StepRequestOptions,
    };

    use super::*;

    fn request(name: &str, mergeable: bool, turn_scoped: bool) -> Arc<dyn StepRequest> {
        Arc::new(ContinuationStepRequest::new(MessageStepRequestOptions {
            request: StepRequestOptions {
                mergeable: Some(mergeable),
                turn_scoped: Some(turn_scoped),
                admission: None,
            },
            kind: Some(name.into()),
        }))
    }

    fn kinds(items: &[Arc<dyn StepRequest>]) -> Vec<&str> {
        items.iter().map(|item| item.kind()).collect()
    }

    #[test]
    fn batches_first_driver_with_all_mergeable_requests_in_queue_order() {
        let mut queue = StepRequestQueue::new();
        for item in [
            request("steer-before", true, false),
            request("driver-a", false, true),
            request("steer-after", true, false),
            request("driver-b", false, true),
            request("steer-last", true, false),
        ] {
            queue.enqueue(item, StepRequestQueuePosition::Tail);
        }
        let first = queue.take_next_batch().unwrap();
        assert_eq!(first.driver.kind(), "driver-a");
        assert_eq!(
            kinds(&first.merged),
            ["steer-before", "steer-after", "steer-last"]
        );
        let second = queue.take_next_batch().unwrap();
        assert_eq!(second.driver.kind(), "driver-b");
        assert!(second.merged.is_empty());
        assert!(queue.take_next_batch().is_none());
    }

    #[test]
    fn all_mergeable_batch_uses_first_item_as_driver_and_head_insertion_wins() {
        let mut queue = StepRequestQueue::new();
        queue.enqueue(
            request("tail-a", true, false),
            StepRequestQueuePosition::Tail,
        );
        queue.enqueue(
            request("tail-b", true, false),
            StepRequestQueuePosition::Tail,
        );
        queue.enqueue(request("head", true, false), StepRequestQueuePosition::Head);
        let batch = queue.take_next_batch().unwrap();
        assert_eq!(batch.driver.kind(), "head");
        assert_eq!(kinds(&batch.merged), ["tail-a", "tail-b"]);
    }

    #[test]
    fn aborted_items_are_lazy_discarded_and_turn_cleanup_keeps_agent_scoped() {
        let aborted = request("aborted", false, true);
        let turn = request("turn", false, true);
        let agent = request("agent", true, false);
        let mut queue = StepRequestQueue::new();
        queue.enqueue(Arc::clone(&aborted), StepRequestQueuePosition::Tail);
        queue.enqueue(Arc::clone(&turn), StepRequestQueuePosition::Tail);
        queue.enqueue(Arc::clone(&agent), StepRequestQueuePosition::Tail);
        assert!(aborted.abort());
        assert!(queue.has_pending_requests());
        queue.abort_turn_scoped();
        assert!(turn.aborted());
        assert!(!agent.aborted());
        let remaining = queue.drain();
        assert_eq!(kinds(&remaining), ["agent"]);
        assert!(!queue.has_pending_requests());
    }

    #[test]
    fn explicit_drain_returns_aborted_entries_without_filtering() {
        let item = request("aborted", false, true);
        let mut queue = StepRequestQueue::new();
        queue.enqueue(Arc::clone(&item), StepRequestQueuePosition::Tail);
        item.abort();
        let drained = queue.drain();
        assert_eq!(kinds(&drained), ["aborted"]);
        assert!(drained[0].aborted());
    }
}
