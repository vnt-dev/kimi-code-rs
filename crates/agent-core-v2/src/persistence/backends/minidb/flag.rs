//! Feature flag for the MiniDB-backed derived read model.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/minidb/flag.ts`.

use std::sync::LazyLock;

use crate::app::flag::{FlagDefinitionInput, FlagSurface, register_flag_definition};

pub const PERSISTENCE_MINIDB_READ_MODEL_FLAG_ID: &str = "persistence_minidb_readmodel";
pub const PERSISTENCE_MINIDB_READ_MODEL_FLAG_ENV: &str =
    "KIMI_CODE_EXPERIMENTAL_PERSISTENCE_MINIDB_READMODEL";

pub static PERSISTENCE_MINIDB_READ_MODEL_FLAG: LazyLock<FlagDefinitionInput> = LazyLock::new(
    || {
        FlagDefinitionInput {
        id: PERSISTENCE_MINIDB_READ_MODEL_FLAG_ID.into(),
        title: "minidb read model".into(),
        description: "Use the minidb-backed IQueryStore as a derived read model for session indexing and wire replay.".into(),
        env: PERSISTENCE_MINIDB_READ_MODEL_FLAG_ENV.into(),
        default: false,
        surface: FlagSurface::Core,
    }
    },
);

pub fn register_persistence_minidb_read_model_flag() {
    register_flag_definition(PERSISTENCE_MINIDB_READ_MODEL_FLAG.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_matches_the_source_contribution() {
        assert_eq!(
            PERSISTENCE_MINIDB_READ_MODEL_FLAG.id,
            PERSISTENCE_MINIDB_READ_MODEL_FLAG_ID
        );
        assert_eq!(
            PERSISTENCE_MINIDB_READ_MODEL_FLAG.env,
            PERSISTENCE_MINIDB_READ_MODEL_FLAG_ENV
        );
        assert_eq!(
            PERSISTENCE_MINIDB_READ_MODEL_FLAG.title,
            "minidb read model"
        );
        assert!(!PERSISTENCE_MINIDB_READ_MODEL_FLAG.default);
        assert_eq!(
            PERSISTENCE_MINIDB_READ_MODEL_FLAG.surface,
            FlagSurface::Core
        );
    }
}
