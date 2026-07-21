use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalFeatureState {
    pub id: String,
    pub enabled: bool,
}

type ExperimentalFlagMap = HashMap<String, bool>;

fn snapshot() -> &'static RwLock<ExperimentalFlagMap> {
    static SNAPSHOT: OnceLock<RwLock<ExperimentalFlagMap>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Original:
///   apps/kimi-code/src/tui/commands/experimental-flags.ts
///   setExperimentalFeatures()
pub fn set_experimental_features(features: &[ExperimentalFeatureState]) {
    let next = features
        .iter()
        .map(|feature| (feature.id.clone(), feature.enabled))
        .collect();
    let mut current = match snapshot().write() {
        Ok(current) => current,
        Err(poisoned) => poisoned.into_inner(),
    };
    *current = next;
}

/// An absent flag ID means the command is not gated and is always enabled.
pub fn is_experimental_flag_enabled(flag: Option<&str>) -> bool {
    let Some(flag) = flag else {
        return true;
    };
    let current = match snapshot().read() {
        Ok(current) => current,
        Err(poisoned) => poisoned.into_inner(),
    };
    current.get(flag) == Some(&true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_snapshot_and_enables_only_explicit_true_flags() {
        set_experimental_features(&[
            ExperimentalFeatureState {
                id: "micro_compaction".to_owned(),
                enabled: true,
            },
            ExperimentalFeatureState {
                id: "other".to_owned(),
                enabled: false,
            },
        ]);
        assert!(is_experimental_flag_enabled(None));
        assert!(is_experimental_flag_enabled(Some("micro_compaction")));
        assert!(!is_experimental_flag_enabled(Some("other")));
        assert!(!is_experimental_flag_enabled(Some("missing")));

        set_experimental_features(&[]);
        assert!(!is_experimental_flag_enabled(Some("micro_compaction")));
    }
}
