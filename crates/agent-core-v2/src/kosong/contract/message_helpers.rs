// Original:
//   packages/agent-core-v2/src/kosong/contract/messageHelpers.ts
//
// Compatibility surface: implementations live beside the wire types in
// `message`, while callers that only need runtime helpers can import this
// narrower module just as they do in the TypeScript package.
pub use super::message::{
    create_assistant_message, create_tool_message, create_user_message, extract_text,
    is_content_part, is_tool_call, is_tool_call_part, is_tool_declaration_only_message,
    merge_in_place,
};
