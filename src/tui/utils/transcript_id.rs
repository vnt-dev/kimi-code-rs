use std::sync::atomic::{AtomicU64, Ordering};

static TRANSCRIPT_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Original:
///   apps/kimi-code/src/tui/utils/transcript-id.ts
///   nextTranscriptId()
pub fn next_transcript_id() -> String {
    let id = TRANSCRIPT_ID_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    format!("entry-{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_monotonically_increasing_prefixed_ids() {
        let first = next_transcript_id();
        let second = next_transcript_id();
        let first_number = first
            .strip_prefix("entry-")
            .and_then(|id| id.parse::<u64>().ok());
        let second_number = second
            .strip_prefix("entry-")
            .and_then(|id| id.parse::<u64>().ok());

        assert!(
            matches!((first_number, second_number), (Some(first), Some(second)) if second == first + 1)
        );
    }
}
