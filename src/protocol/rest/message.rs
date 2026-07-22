use serde::{Deserialize, Serialize};

use crate::protocol::{CursorQuery, Message, MessageRole};

// Original: rest/message.ts, cursorQuerySchema.and({ role }).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListMessagesQuery {
    #[serde(flatten)]
    pub cursor: CursorQuery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<MessageRole>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListMessagesResponse {
    pub items: Vec<Message>,
    pub has_more: bool,
}

pub type GetMessageResponse = Message;
