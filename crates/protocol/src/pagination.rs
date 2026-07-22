use serde::{Deserialize, Deserializer, Serialize};
use std::error::Error;
use std::fmt;

use super::error_codes::ErrorCode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CursorQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i64>,
}

#[derive(Deserialize)]
struct RawCursorQuery {
    before_id: Option<String>,
    after_id: Option<String>,
    page_size: Option<i64>,
}

impl<'de> Deserialize<'de> for CursorQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawCursorQuery::deserialize(deserializer)?;
        let query = Self {
            before_id: raw.before_id,
            after_id: raw.after_id,
            page_size: raw.page_size,
        };
        query.validate().map_err(serde::de::Error::custom)?;
        Ok(query)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginationValidationError {
    pub code: ErrorCode,
    pub message: &'static str,
    pub path: &'static str,
}

impl fmt::Display for PaginationValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for PaginationValidationError {}

// Original: pagination.ts, cursorQuerySchema.superRefine()
impl CursorQuery {
    pub fn validate(&self) -> Result<(), PaginationValidationError> {
        if self.before_id.as_ref().is_some_and(String::is_empty) {
            return Err(PaginationValidationError {
                code: ErrorCode::ValidationFailed,
                message: "before_id must not be empty",
                path: "before_id",
            });
        }
        if self.after_id.as_ref().is_some_and(String::is_empty) {
            return Err(PaginationValidationError {
                code: ErrorCode::ValidationFailed,
                message: "after_id must not be empty",
                path: "after_id",
            });
        }
        if self.before_id.is_some() && self.after_id.is_some() {
            return Err(PaginationValidationError {
                code: ErrorCode::ValidationFailed,
                message: "before_id and after_id are mutually exclusive",
                path: "before_id",
            });
        }
        if self
            .page_size
            .is_some_and(|size| !(1..=100).contains(&size))
        {
            return Err(PaginationValidationError {
                code: ErrorCode::ValidationFailed,
                message: "page_size must be between 1 and 100",
                path: "page_size",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageResponse<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}
