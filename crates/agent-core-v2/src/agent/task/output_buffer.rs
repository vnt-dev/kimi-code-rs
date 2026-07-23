//! Bounded in-memory and pending-persistence output state for agent tasks.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `appendOutput()`, `startOutputPersist()`, and `appendRetainedOutput()`.

use std::collections::VecDeque;

use super::{MAX_RETAINED_OUTPUT_BYTES, MAX_TASK_OUTPUT_BYTES, output_limit_reason};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOutputAction {
    None,
    AppendPersisted(String),
    StartPersisting(String),
    StopProcess(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskOutputBuffer {
    output_chunks: VecDeque<String>,
    pub output_size_bytes: usize,
    retained_output_bytes: usize,
    pub output_limit_tripped: bool,
    pending_output: Vec<String>,
    pending_output_bytes: usize,
    pub output_persist_started: bool,
}

impl TaskOutputBuffer {
    pub fn new(output_persist_started: bool) -> Self {
        Self {
            output_persist_started,
            ..Self::default()
        }
    }

    pub fn retained_output(&self) -> String {
        self.output_chunks.iter().cloned().collect()
    }

    pub fn retained_output_bytes(&self) -> usize {
        self.retained_output_bytes
    }

    pub fn pending_output_bytes(&self) -> usize {
        self.pending_output_bytes
    }

    // Original: taskService.ts, settleTask() non-persisted output branch.
    pub fn discard_pending_output(&mut self) {
        self.pending_output.clear();
        self.pending_output_bytes = 0;
    }

    // Original: taskService.ts, appendOutput().
    pub fn append(&mut self, chunk: String, is_process: bool) -> TaskOutputAction {
        let chunk_bytes = chunk.len();
        self.output_size_bytes = self.output_size_bytes.saturating_add(chunk_bytes);
        self.append_retained_output(&chunk, chunk_bytes);

        if !self.output_limit_tripped
            && is_process
            && self.output_size_bytes > MAX_TASK_OUTPUT_BYTES
        {
            self.output_limit_tripped = true;
            return TaskOutputAction::StopProcess(output_limit_reason());
        }
        if self.output_limit_tripped {
            return TaskOutputAction::None;
        }
        if !self.output_persist_started {
            self.pending_output.push(chunk);
            self.pending_output_bytes = self.pending_output_bytes.saturating_add(chunk_bytes);
            if self.pending_output_bytes > MAX_RETAINED_OUTPUT_BYTES {
                return self
                    .start_output_persist()
                    .map_or(TaskOutputAction::None, TaskOutputAction::StartPersisting);
            }
            return TaskOutputAction::None;
        }
        TaskOutputAction::AppendPersisted(chunk)
    }

    // Original: taskService.ts, startOutputPersist(). The caller appends the
    // returned concatenated block to the task output write queue.
    pub fn start_output_persist(&mut self) -> Option<String> {
        if self.output_persist_started {
            return None;
        }
        self.output_persist_started = true;
        let pending = (!self.pending_output.is_empty()).then(|| self.pending_output.join(""));
        self.pending_output.clear();
        self.pending_output_bytes = 0;
        pending
    }

    // Original: taskService.ts, appendRetainedOutput(). Byte slicing followed
    // by UTF-8 lossy decoding matches Buffer.subarray().toString('utf-8').
    fn append_retained_output(&mut self, chunk: &str, chunk_bytes: usize) {
        if chunk_bytes >= MAX_RETAINED_OUTPUT_BYTES {
            let retained = String::from_utf8_lossy(
                &chunk.as_bytes()[chunk_bytes - MAX_RETAINED_OUTPUT_BYTES..],
            )
            .into_owned();
            self.output_chunks.clear();
            self.retained_output_bytes = retained.len();
            self.output_chunks.push_back(retained);
            return;
        }

        self.output_chunks.push_back(chunk.to_owned());
        self.retained_output_bytes = self.retained_output_bytes.saturating_add(chunk_bytes);
        while self.retained_output_bytes > MAX_RETAINED_OUTPUT_BYTES {
            let Some(removed) = self.output_chunks.pop_front() else {
                break;
            };
            self.retained_output_bytes = self.retained_output_bytes.saturating_sub(removed.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_output_flushes_only_after_strictly_exceeding_one_mib() {
        let mut output = TaskOutputBuffer::new(false);
        let exact = "a".repeat(MAX_RETAINED_OUTPUT_BYTES);
        assert_eq!(output.append(exact.clone(), false), TaskOutputAction::None);
        assert!(!output.output_persist_started);
        assert_eq!(output.pending_output_bytes(), MAX_RETAINED_OUTPUT_BYTES);

        assert_eq!(
            output.append("b".into(), false),
            TaskOutputAction::StartPersisting(format!("{exact}b"))
        );
        assert!(output.output_persist_started);
        assert_eq!(output.pending_output_bytes(), 0);
        assert_eq!(
            output.append("next".into(), false),
            TaskOutputAction::AppendPersisted("next".into())
        );
        assert_eq!(output.start_output_persist(), None);
    }

    #[test]
    fn retained_output_drops_whole_old_chunks_and_lossily_slices_large_chunks() {
        let mut output = TaskOutputBuffer::new(true);
        assert_eq!(
            output.append("old".into(), false),
            TaskOutputAction::AppendPersisted("old".into())
        );
        let large = format!("€{}", "x".repeat(MAX_RETAINED_OUTPUT_BYTES - 2));
        assert_eq!(large.len(), MAX_RETAINED_OUTPUT_BYTES + 1);
        assert!(matches!(
            output.append(large, false),
            TaskOutputAction::AppendPersisted(_)
        ));
        assert!(output.retained_output().starts_with("\u{fffd}\u{fffd}"));
        assert_eq!(
            output.retained_output_bytes(),
            MAX_RETAINED_OUTPUT_BYTES + 4
        );
        assert!(!output.retained_output().contains("old"));
    }

    #[test]
    fn process_limit_stops_once_after_strictly_exceeding_sixteen_mib() {
        let mut output = TaskOutputBuffer::new(true);
        let exact = "x".repeat(MAX_TASK_OUTPUT_BYTES);
        assert_eq!(
            output.append(exact, true),
            TaskOutputAction::AppendPersisted("x".repeat(MAX_TASK_OUTPUT_BYTES))
        );
        assert_eq!(
            output.append("!".into(), true),
            TaskOutputAction::StopProcess(output_limit_reason())
        );
        assert_eq!(
            output.append("ignored".into(), true),
            TaskOutputAction::None
        );
        assert_eq!(output.output_size_bytes, MAX_TASK_OUTPUT_BYTES + 8);

        let mut agent_output = TaskOutputBuffer::new(true);
        assert_eq!(
            agent_output.append("x".repeat(MAX_TASK_OUTPUT_BYTES + 1), false),
            TaskOutputAction::AppendPersisted("x".repeat(MAX_TASK_OUTPUT_BYTES + 1))
        );
        assert!(!agent_output.output_limit_tripped);
    }

    #[test]
    fn explicit_persist_start_flushes_pending_in_original_order() {
        let mut output = TaskOutputBuffer::new(false);
        output.append("one".into(), false);
        output.append("two".into(), false);
        assert_eq!(output.start_output_persist(), Some("onetwo".into()));
        assert_eq!(output.pending_output_bytes(), 0);
    }

    #[test]
    fn settlement_can_discard_unpersisted_output_without_changing_retained_tail() {
        let mut output = TaskOutputBuffer::new(false);
        output.append("visible tail".into(), false);
        output.discard_pending_output();
        assert_eq!(output.pending_output_bytes(), 0);
        assert_eq!(output.retained_output(), "visible tail");
        assert!(!output.output_persist_started);
    }
}
