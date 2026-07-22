use serde::{Deserialize, Serialize};

use crate::validation::{literal_false, literal_true};
use crate::{ApprovalRequest, ApprovalResponse, IsoDateTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingApprovalStatus {
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingApprovalsQuery {
    pub status: PendingApprovalStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListPendingApprovalsResponse {
    pub items: Vec<ApprovalRequest>,
}

pub type ApprovalResolveRequest = ApprovalResponse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResolveResult {
    #[serde(deserialize_with = "literal_true")]
    pub resolved: bool,
    pub resolved_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAlreadyResolvedData {
    #[serde(deserialize_with = "literal_false")]
    pub resolved: bool,
}
