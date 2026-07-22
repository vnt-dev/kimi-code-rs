use serde::{Deserialize, Serialize};

use crate::validation::{non_empty, required_nullable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedProviderStatus {
    Authenticated,
    Expired,
    Revoked,
    Unauthenticated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProviderSummary {
    #[serde(deserialize_with = "non_empty")]
    pub name: String,
    pub status: ManagedProviderStatus,
}

// Original: rest/auth.ts, authSummarySchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSummary {
    pub ready: bool,
    pub providers_count: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub default_model: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub managed_provider: Option<ManagedProviderSummary>,
}
