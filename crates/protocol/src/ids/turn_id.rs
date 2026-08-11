use super::unsigned_id::define_unsigned_id;

define_unsigned_id!(
    /// Canonical identifier for an agent-loop turn.
    ///
    /// Runtime turn identifiers are non-negative integral counters. Transcript
    /// identifiers (for example, `"t7"`) remain a separate type because they
    /// belong to the transcript model.
    TurnId,
    "turn id",
    true
);

/// Serde adapter for historical records whose turn id is written as a string.
pub mod string_option {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::TurnId;

    pub fn serialize<S>(value: &Option<TurnId>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(turn_id) => serializer.serialize_some(&turn_id.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<TurnId>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<TurnId>::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct LegacyRecord {
        #[serde(with = "string_option")]
        turn_id: Option<TurnId>,
    }

    #[test]
    fn legacy_string_adapter_preserves_wire_shape_and_accepts_old_values() {
        let record = LegacyRecord {
            turn_id: Some(TurnId::new(7)),
        };
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            json!({"turn_id": "7"})
        );
        assert_eq!(
            serde_json::from_value::<LegacyRecord>(json!({"turn_id": " 3.8"})).unwrap(),
            LegacyRecord {
                turn_id: Some(TurnId::new(3))
            }
        );
    }

    #[test]
    fn deserializes_non_negative_integer_float_and_prefixed_string_representations() {
        for (value, expected) in [
            (json!(7), 7),
            (json!(7.9), 7),
            (json!("100"), 100),
            (json!("t100"), 100),
            (json!("t100.9"), 100),
        ] {
            assert_eq!(
                serde_json::from_value::<TurnId>(value).unwrap(),
                TurnId::new(expected)
            );
        }
        for value in [json!(-1), json!(-7.9), json!("-1"), json!("t-1")] {
            assert!(serde_json::from_value::<TurnId>(value).is_err());
        }
        assert!(serde_json::from_value::<TurnId>(json!("turn-1")).is_err());
    }
}
