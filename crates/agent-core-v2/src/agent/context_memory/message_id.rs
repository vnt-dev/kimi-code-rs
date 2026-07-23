use ulid::Ulid;

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/messageId.ts
//   newMessageId()
//
// Local message ids are process-lifetime identifiers. Provider-assigned ids
// use a separate field and persisted public ids are derived from transcript
// indexes, matching the original namespace separation.
pub fn new_message_id() -> String {
    format!("msg_{}", Ulid::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_prefixed_ulid() {
        let id = new_message_id();
        let ulid = id
            .strip_prefix("msg_")
            .expect("local message id must retain its namespace prefix");

        assert_eq!(ulid.len(), 26);
        assert_eq!(ulid, ulid.to_ascii_uppercase());
        assert!(ulid.parse::<Ulid>().is_ok());
    }

    #[test]
    fn creates_distinct_ids() {
        assert_ne!(new_message_id(), new_message_id());
    }
}
