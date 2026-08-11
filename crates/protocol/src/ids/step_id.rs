use super::unsigned_id::define_unsigned_id;

define_unsigned_id!(
    /// Canonical numeric identifier for a step within an agent-loop turn.
    StepId,
    "step id",
    false
);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serializes_as_an_integer_and_reads_legacy_numeric_shapes() {
        assert_eq!(serde_json::to_value(StepId::new(7)).unwrap(), json!(7));
        for (value, expected) in [
            (json!(7), 7),
            (json!(7.9), 7),
            (json!("7"), 7),
            (json!("7.9"), 7),
        ] {
            assert_eq!(
                serde_json::from_value::<StepId>(value).unwrap(),
                StepId::new(expected)
            );
        }
        assert!(serde_json::from_value::<StepId>(json!(-1)).is_err());
        assert!(serde_json::from_value::<StepId>(json!(-1.0)).is_err());
        assert!(serde_json::from_value::<StepId>(json!("s1")).is_err());
    }
}
