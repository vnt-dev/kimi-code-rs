//! Model-requester implementation helpers.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/modelRequesterImpl.ts`.

use std::time::Instant;

use crate::kosong::contract::provider::StreamDecodeStats;

use super::model_requester::ModelRequestTiming;

// Original: buildStreamTiming(). `Instant` replaces the source's wall-clock
// `Date.now()` values; callers capture all timestamps on the same monotonic
// clock so clock adjustments cannot produce externally visible negative spans.
pub fn build_stream_timing(
    request_started_at: Instant,
    request_sent_at: Option<Instant>,
    first_chunk_at: Instant,
    stream_ended_at: Option<Instant>,
    decode_stats: Option<StreamDecodeStats>,
) -> ModelRequestTiming {
    let output_ended_at = stream_ended_at.unwrap_or_else(Instant::now);
    let first_token_latency_ms = first_chunk_at
        .checked_duration_since(request_started_at)
        .map_or(0.0, |duration| duration.as_secs_f64() * 1_000.0);
    let stream_duration_ms = output_ended_at
        .checked_duration_since(first_chunk_at)
        .map_or(0.0, |duration| duration.as_secs_f64() * 1_000.0);
    let (request_build_ms, server_first_token_ms) = request_sent_at.map_or((None, None), |sent| {
        // Original: min(max(requestSentAt, requestStartedAt), firstChunkAt).
        let clamped_sent_at = sent.clamp(request_started_at, first_chunk_at);
        (
            Some(
                clamped_sent_at
                    .duration_since(request_started_at)
                    .as_secs_f64()
                    * 1_000.0,
            ),
            Some(first_chunk_at.duration_since(clamped_sent_at).as_secs_f64() * 1_000.0),
        )
    });

    ModelRequestTiming {
        first_token_latency_ms,
        stream_duration_ms,
        request_build_ms,
        server_first_token_ms,
        server_decode_ms: decode_stats.map(|stats| stats.server_decode_ms.max(0.0)),
        client_consume_ms: decode_stats.map(|stats| stats.client_consume_ms.max(0.0)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn timing_clamps_sent_timestamp_and_decode_metrics_like_the_source() {
        let started = Instant::now();
        let first = started + Duration::from_millis(30);
        let ended = first + Duration::from_millis(40);
        let timing = build_stream_timing(
            started,
            Some(started + Duration::from_millis(90)),
            first,
            Some(ended),
            Some(StreamDecodeStats {
                server_decode_ms: -5.0,
                client_consume_ms: 7.0,
            }),
        );
        assert_eq!(timing.first_token_latency_ms, 30.0);
        assert_eq!(timing.stream_duration_ms, 40.0);
        assert_eq!(timing.request_build_ms, Some(30.0));
        assert_eq!(timing.server_first_token_ms, Some(0.0));
        assert_eq!(timing.server_decode_ms, Some(0.0));
        assert_eq!(timing.client_consume_ms, Some(7.0));
    }

    #[test]
    fn timing_clamps_pre_start_send_time_and_keeps_metrics_absent_when_unreported() {
        let started = Instant::now();
        let first = started + Duration::from_millis(20);
        let timing = build_stream_timing(
            started,
            Some(started - Duration::from_millis(1)),
            first,
            Some(first),
            None,
        );
        assert_eq!(timing.request_build_ms, Some(0.0));
        assert_eq!(timing.server_first_token_ms, Some(20.0));
        assert_eq!(timing.server_decode_ms, None);
        assert_eq!(timing.client_consume_ms, None);
    }
}
