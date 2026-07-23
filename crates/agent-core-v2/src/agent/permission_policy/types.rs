use serde::{Deserialize, Serialize};

// Original:
//   packages/agent-core-v2/src/agent/permissionPolicy/types.ts
//   PermissionMode
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    #[default]
    Manual,
    Yolo,
    Auto,
}
