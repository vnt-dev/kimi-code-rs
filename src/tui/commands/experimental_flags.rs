use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagSurface {
    Core,
    Tui,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentalFlagSource {
    MasterEnv,
    Env,
    Config,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalFeatureState {
    pub id: String,
    pub title: String,
    pub description: String,
    pub surface: FlagSurface,
    pub env: String,
    pub default_enabled: bool,
    pub enabled: bool,
    pub source: ExperimentalFlagSource,
    pub config_value: Option<bool>,
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
                title: "Micro compaction".to_owned(),
                description: String::new(),
                surface: FlagSurface::Core,
                env: "KIMI_CODE_MICRO_COMPACTION".to_owned(),
                default_enabled: false,
                enabled: true,
                source: ExperimentalFlagSource::Config,
                config_value: Some(true),
            },
            ExperimentalFeatureState {
                id: "other".to_owned(),
                title: "Other".to_owned(),
                description: String::new(),
                surface: FlagSurface::Tui,
                env: "KIMI_CODE_OTHER".to_owned(),
                default_enabled: false,
                enabled: false,
                source: ExperimentalFlagSource::Default,
                config_value: None,
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
