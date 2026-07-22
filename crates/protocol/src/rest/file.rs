use serde::{Deserialize, Serialize};

use crate::FileMeta;
use crate::validation::{literal_true, non_empty};

pub type UploadFileResponse = FileMeta;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetFileParam {
    #[serde(deserialize_with = "non_empty")]
    pub file_id: String,
}

pub type DeleteFileParam = GetFileParam;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteFileResponse {
    #[serde(deserialize_with = "literal_true")]
    pub deleted: bool,
}
