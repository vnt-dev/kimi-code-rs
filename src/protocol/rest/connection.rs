use serde::{Deserialize, Serialize};

use crate::protocol::time::IsoDateTime;
use crate::protocol::validation::{non_empty, required_nullable};

// Original: rest/connection.ts, connectionSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    pub connected_at: IsoDateTime,
    #[serde(deserialize_with = "required_nullable")]
    pub remote_address: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub user_agent: Option<String>,
    pub has_client_hello: bool,
    pub subscriptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionsListResponse {
    pub connections: Vec<Connection>,
}
