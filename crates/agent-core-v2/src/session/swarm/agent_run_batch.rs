//! Session swarm batch-scheduler configuration.
//!
//! Original: `session/swarm/agentRunBatch.ts`, `resolveSwarmMaxConcurrency()`.

use std::collections::HashMap;

pub const AGENT_SWARM_MAX_CONCURRENCY_ENV: &str = "KIMI_CODE_AGENT_SWARM_MAX_CONCURRENCY";

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{AGENT_SWARM_MAX_CONCURRENCY_ENV} must be a positive integer, got {raw:?}.")]
pub struct SwarmMaxConcurrencyError {
    pub raw: String,
}

pub fn resolve_swarm_max_concurrency(
    env: &HashMap<String, String>,
) -> Result<Option<usize>, SwarmMaxConcurrencyError> {
    let Some(raw) = env.get(AGENT_SWARM_MAX_CONCURRENCY_ENV) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value = raw
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| SwarmMaxConcurrencyError { raw: raw.clone() })?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_optional_positive_concurrency_and_preserves_invalid_error() {
        assert_eq!(
            resolve_swarm_max_concurrency(&HashMap::new()).unwrap(),
            None
        );
        let valid = HashMap::from([(AGENT_SWARM_MAX_CONCURRENCY_ENV.into(), " 12 ".into())]);
        assert_eq!(resolve_swarm_max_concurrency(&valid).unwrap(), Some(12));
        let invalid = HashMap::from([(AGENT_SWARM_MAX_CONCURRENCY_ENV.into(), "1.5".into())]);
        assert_eq!(
            resolve_swarm_max_concurrency(&invalid)
                .unwrap_err()
                .to_string(),
            "KIMI_CODE_AGENT_SWARM_MAX_CONCURRENCY must be a positive integer, got \"1.5\"."
        );
    }
}
