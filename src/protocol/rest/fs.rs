use serde::{Deserialize, Serialize};

// Original: rest/fs.ts, fsOpenInAppIdSchema.
// The remaining filesystem REST payloads are migrated in a later unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsOpenInAppId {
    Finder,
    Cursor,
    Vscode,
    Iterm,
    Terminal,
}
