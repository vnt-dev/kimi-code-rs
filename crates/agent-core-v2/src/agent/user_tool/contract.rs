//! User-defined tool registration contracts.
//!
//! Original: `agent/userTool/userTool.ts`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserToolRegistration {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
}
