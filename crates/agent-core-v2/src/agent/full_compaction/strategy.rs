use std::sync::Arc;

use crate::{
    agent::{full_compaction::CompactionSource, profile::ProfileModelContext},
    kosong::contract::{
        message::{Message, Role},
        tokens::estimate_tokens_for_message,
    },
};

// Original:
//   packages/agent-core-v2/src/agent/fullCompaction/strategy.ts
//   CompactionConfig, RuntimeCompactionStrategy, DefaultCompactionStrategy
//
// Rust adaptation:
//   Context suppliers are owned `Arc` closures so a strategy can retain the
//   source's live model-context lookup without a TypeScript service container.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompactionConfig {
    pub trigger_ratio: f64,
    pub block_ratio: f64,
    pub reserved_context_size: f64,
    pub max_compaction_per_turn: f64,
    pub max_overflow_compaction_attempts: f64,
    pub max_recent_messages: f64,
    pub max_recent_user_messages: f64,
    pub max_recent_size_ratio: f64,
    pub min_overflow_reduction_ratio: f64,
}

pub const DEFAULT_COMPACTION_CONFIG: CompactionConfig = CompactionConfig {
    trigger_ratio: 0.85,
    block_ratio: 0.85,
    reserved_context_size: 50_000.0,
    max_compaction_per_turn: f64::INFINITY,
    max_overflow_compaction_attempts: 3.0,
    max_recent_messages: 4.0,
    max_recent_user_messages: f64::INFINITY,
    max_recent_size_ratio: 0.2,
    min_overflow_reduction_ratio: 0.05,
};

pub trait CompactionStrategy: Send + Sync {
    fn should_compact(&self, used_size: f64) -> bool;
    fn should_block(&self, used_size: f64) -> bool;
    fn compute_compact_count(&self, messages: &[Message], source: CompactionSource) -> usize;
    fn reduce_compact_on_overflow(&self, messages: &[Message]) -> usize;
    fn check_after_step(&self) -> bool;
    fn max_compaction_per_turn(&self) -> f64;
    fn max_overflow_compaction_attempts(&self) -> f64;
}

pub type ProfileModelContextProvider = Arc<dyn Fn() -> ProfileModelContext + Send + Sync>;
pub type MaxSizeProvider = Arc<dyn Fn() -> f64 + Send + Sync>;

pub struct RuntimeCompactionStrategy {
    context: ProfileModelContextProvider,
}

impl RuntimeCompactionStrategy {
    pub fn new(context: ProfileModelContextProvider) -> Self {
        Self { context }
    }

    fn config(&self, model: &ProfileModelContext) -> CompactionConfig {
        let trigger_ratio = model
            .compaction_trigger_ratio
            .unwrap_or(DEFAULT_COMPACTION_CONFIG.trigger_ratio);
        CompactionConfig {
            trigger_ratio,
            block_ratio: trigger_ratio.max(DEFAULT_COMPACTION_CONFIG.block_ratio),
            reserved_context_size: model
                .reserved_context_size
                .map_or(DEFAULT_COMPACTION_CONFIG.reserved_context_size, |size| {
                    size as f64
                }),
            ..DEFAULT_COMPACTION_CONFIG
        }
    }

    fn delegate(&self) -> DefaultCompactionStrategy {
        let model = (self.context)();
        let max_size = model.model_capabilities.max_context_tokens as f64;
        DefaultCompactionStrategy::new(Arc::new(move || max_size), self.config(&model))
    }

    fn window_delegate(&self) -> DefaultCompactionStrategy {
        let model = (self.context)();
        let max_size = model.model_capabilities.max_context_tokens as f64;
        DefaultCompactionStrategy::new(Arc::new(move || max_size), DEFAULT_COMPACTION_CONFIG)
    }
}

impl CompactionStrategy for RuntimeCompactionStrategy {
    fn should_compact(&self, used_size: f64) -> bool {
        self.delegate().should_compact(used_size)
    }

    fn should_block(&self, used_size: f64) -> bool {
        self.delegate().should_block(used_size)
    }

    fn compute_compact_count(&self, messages: &[Message], source: CompactionSource) -> usize {
        self.window_delegate()
            .compute_compact_count(messages, source)
    }

    fn reduce_compact_on_overflow(&self, messages: &[Message]) -> usize {
        self.window_delegate().reduce_compact_on_overflow(messages)
    }

    fn check_after_step(&self) -> bool {
        let trigger_ratio = self.config(&(self.context)()).trigger_ratio;
        let block_ratio = self.config(&(self.context)()).block_ratio;
        trigger_ratio != block_ratio
    }

    fn max_compaction_per_turn(&self) -> f64 {
        DEFAULT_COMPACTION_CONFIG.max_compaction_per_turn
    }

    fn max_overflow_compaction_attempts(&self) -> f64 {
        DEFAULT_COMPACTION_CONFIG.max_overflow_compaction_attempts
    }
}

pub struct DefaultCompactionStrategy {
    max_size_provider: MaxSizeProvider,
    config: CompactionConfig,
}

impl DefaultCompactionStrategy {
    pub fn new(max_size_provider: MaxSizeProvider, config: CompactionConfig) -> Self {
        Self {
            max_size_provider,
            config,
        }
    }

    pub fn with_max_size(max_size: f64, config: CompactionConfig) -> Self {
        Self::new(Arc::new(move || max_size), config)
    }

    fn max_size(&self) -> f64 {
        (self.max_size_provider)()
    }

    fn should_use_reserved_context(&self, used_size: f64) -> bool {
        let max_size = self.max_size();
        let reserved_size = self.config.reserved_context_size;
        reserved_size > 0.0 && reserved_size < max_size && used_size + reserved_size >= max_size
    }

    fn fit_compact_count_to_window(&self, messages: &[Message], compacted_count: usize) -> usize {
        let max_size = self.max_size();
        if max_size <= 0.0 || compacted_count == 0 {
            return compacted_count;
        }

        let mut compacted_size = messages[..compacted_count]
            .iter()
            .map(estimate_tokens_for_message)
            .sum::<usize>() as f64;
        if compacted_size <= max_size {
            return compacted_count;
        }

        let mut best_count = None;
        for count in (1..compacted_count).rev() {
            compacted_size -= estimate_tokens_for_message(&messages[count]) as f64;
            if !can_split_after(messages, count - 1) {
                continue;
            }
            best_count = Some(count);
            if compacted_size <= max_size {
                return count;
            }
        }

        best_count.unwrap_or(compacted_count)
    }
}

impl CompactionStrategy for DefaultCompactionStrategy {
    fn should_compact(&self, used_size: f64) -> bool {
        let max_size = self.max_size();
        max_size > 0.0
            && (used_size >= max_size * self.config.trigger_ratio
                || self.should_use_reserved_context(used_size))
    }

    fn should_block(&self, used_size: f64) -> bool {
        let max_size = self.max_size();
        max_size > 0.0
            && (used_size >= max_size * self.config.block_ratio
                || self.should_use_reserved_context(used_size))
    }

    fn compute_compact_count(&self, messages: &[Message], source: CompactionSource) -> usize {
        if source == CompactionSource::Manual {
            for index in (1..messages.len()).rev() {
                if can_split_after(messages, index) {
                    return self.fit_compact_count_to_window(messages, index + 1);
                }
            }
            return 0;
        }

        let mut recent_messages = 1usize;
        let mut recent_user_messages = 0usize;
        let mut recent_size = 0usize;
        let mut best_count = None;

        while recent_messages < messages.len() {
            let split_index = messages.len() - recent_messages - 1;
            let message = &messages[messages.len() - recent_messages];

            if message.role == Role::User {
                recent_user_messages += 1;
            }
            recent_size += estimate_tokens_for_message(message);

            if can_split_after(messages, split_index) {
                best_count = Some(split_index + 1);
            }

            let reaches_max = recent_messages as f64 >= self.config.max_recent_messages
                || recent_user_messages as f64 >= self.config.max_recent_user_messages
                || recent_size as f64 >= self.max_size() * self.config.max_recent_size_ratio;
            if reaches_max && best_count.is_some() {
                break;
            }
            recent_messages += 1;
        }

        self.fit_compact_count_to_window(messages, best_count.unwrap_or(0))
    }

    fn reduce_compact_on_overflow(&self, messages: &[Message]) -> usize {
        let min_reduced_size = (self.max_size() * self.config.min_overflow_reduction_ratio)
            .ceil()
            .max(1.0);
        let mut reduced_size = 0usize;
        let mut best_count = None;

        for index in (1..messages.len().saturating_sub(1)).rev() {
            reduced_size += estimate_tokens_for_message(&messages[index + 1]);
            if can_split_after(messages, index) {
                best_count = Some(index + 1);
                if reduced_size as f64 >= min_reduced_size {
                    return index + 1;
                }
            }
        }
        best_count.unwrap_or(messages.len())
    }

    fn check_after_step(&self) -> bool {
        self.config.trigger_ratio != self.config.block_ratio
    }

    fn max_compaction_per_turn(&self) -> f64 {
        self.config.max_compaction_per_turn
    }

    fn max_overflow_compaction_attempts(&self) -> f64 {
        self.config.max_overflow_compaction_attempts
    }
}

fn can_split_after(messages: &[Message], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if message.role == Role::User
        || (message.role == Role::Assistant && !message.tool_calls.is_empty())
        || messages
            .get(index + 1)
            .is_some_and(|next| next.role == Role::Tool)
    {
        return false;
    }
    !prefix_ends_with_open_tool_exchange(messages, index)
}

fn prefix_ends_with_open_tool_exchange(messages: &[Message], index: usize) -> bool {
    if messages
        .get(index)
        .is_none_or(|message| message.role != Role::Tool)
    {
        return false;
    }

    let mut tool_result_count = 0usize;
    for message in messages[..=index].iter().rev() {
        if message.role == Role::Tool {
            tool_result_count += 1;
            continue;
        }
        return message.role == Role::Assistant && message.tool_calls.len() > tool_result_count;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::{
        ToolCall, ToolCallType, ToolOutput, create_assistant_message, create_tool_message,
        create_user_message,
    };

    fn strategy(max_size: f64) -> DefaultCompactionStrategy {
        DefaultCompactionStrategy::with_max_size(max_size, DEFAULT_COMPACTION_CONFIG)
    }

    fn tool_call(id: &str) -> ToolCall {
        ToolCall {
            call_type: ToolCallType::Function,
            id: id.into(),
            name: "read".into(),
            arguments: Some("{}".into()),
            extras: None,
            stream_index: None,
        }
    }

    #[test]
    fn thresholds_include_reserved_context_and_ignore_non_positive_windows() {
        let compaction_strategy = strategy(100.0);
        assert!(!compaction_strategy.should_compact(84.0));
        assert!(compaction_strategy.should_compact(85.0));

        let reserved_context = DefaultCompactionStrategy::with_max_size(
            100.0,
            CompactionConfig {
                reserved_context_size: 50.0,
                ..DEFAULT_COMPACTION_CONFIG
            },
        );
        assert!(reserved_context.should_block(50.0));

        let disabled = strategy(0.0);
        assert!(!disabled.should_compact(1_000.0));
        assert!(!disabled.should_block(1_000.0));
    }

    #[test]
    fn manual_and_auto_compaction_preserve_valid_message_boundaries() {
        let messages = vec![
            create_user_message("first"),
            create_assistant_message(Vec::new(), None),
            create_user_message("latest"),
        ];
        let strategy = strategy(100.0);
        assert_eq!(
            strategy.compute_compact_count(&messages, CompactionSource::Manual),
            2
        );
        assert_eq!(
            strategy.compute_compact_count(&messages, CompactionSource::Auto),
            2
        );
    }

    #[test]
    fn an_open_tool_exchange_cannot_be_a_compaction_boundary() {
        let messages = vec![
            create_user_message("first"),
            create_assistant_message(Vec::new(), Some(vec![tool_call("one"), tool_call("two")])),
            create_tool_message("one", ToolOutput::Text("done".into())),
        ];
        assert_eq!(
            strategy(100.0).compute_compact_count(&messages, CompactionSource::Manual),
            0
        );
    }

    #[test]
    fn overflow_reduction_falls_back_to_the_complete_message_list_when_unsplittable() {
        let messages = vec![
            create_user_message("first"),
            create_assistant_message(Vec::new(), Some(vec![tool_call("one"), tool_call("two")])),
            create_tool_message("one", ToolOutput::Text("done".into())),
        ];
        assert_eq!(
            strategy(100.0).reduce_compact_on_overflow(&messages),
            messages.len()
        );
    }
}
