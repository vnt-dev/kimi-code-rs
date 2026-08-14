use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Serde support for `Option<u64>` fields on the TypeScript wire, where a
/// `number` may be written as either `100` or `100.0`. Integral floats are
/// accepted, non-integer floats are truncated (token/byte semantics), and
/// negative, non-finite, or out-of-range values are rejected.
pub(crate) mod lenient_u64 {
    use serde::de::{Error, Visitor};

    use super::*;

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<LenientU64>::deserialize(deserializer).map(|value| value.map(|value| value.0))
    }

    struct LenientU64(u64);

    impl<'de> Deserialize<'de> for LenientU64 {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(LenientU64Visitor)
        }
    }

    struct LenientU64Visitor;

    impl Visitor<'_> for LenientU64Visitor {
        type Value = LenientU64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an unsigned integer or an integral float")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
            Ok(LenientU64(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: Error,
        {
            u64::try_from(value).map(LenientU64).map_err(E::custom)
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
        where
            E: Error,
        {
            if !value.is_finite() {
                return Err(E::custom(format!("value must be finite, got {value}")));
            }
            let truncated = value.trunc();
            if truncated < 0.0 || truncated >= u64::MAX as f64 {
                return Err(E::custom(format!(
                    "value is outside the unsigned 64-bit range: {value}"
                )));
            }
            Ok(LenientU64(truncated as u64))
        }
    }
}

/// Serde support for a TypeScript optional field whose present value may
/// itself be JSON `null`.
///
/// `None` means the property was absent, `Some(None)` means it was explicitly
/// `null`, and `Some(Some(value))` carries a non-null value.
pub(crate) mod double_option {
    use super::*;

    pub(crate) fn serialize<S, T>(
        value: &Option<Option<T>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        match value {
            Some(Some(value)) => value.serialize(serializer),
            Some(None) | None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use crate::model::TranscriptUsage;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Lenient {
        #[serde(default, deserialize_with = "super::lenient_u64::deserialize")]
        value: Option<u64>,
    }

    #[test]
    fn lenient_u64_accepts_integers_and_integral_floats() {
        let parsed: Lenient = serde_json::from_str(r#"{"value": 100}"#).unwrap();
        assert_eq!(parsed.value, Some(100));
        let parsed: Lenient = serde_json::from_str(r#"{"value": 100.0}"#).unwrap();
        assert_eq!(parsed.value, Some(100));
    }

    #[test]
    fn lenient_u64_truncates_non_integer_floats() {
        let parsed: Lenient = serde_json::from_str(r#"{"value": 100.9}"#).unwrap();
        assert_eq!(parsed.value, Some(100));
    }

    #[test]
    fn lenient_u64_accepts_missing_and_null() {
        let parsed: Lenient = serde_json::from_str(r"{}").unwrap();
        assert_eq!(parsed.value, None);
        let parsed: Lenient = serde_json::from_str(r#"{"value": null}"#).unwrap();
        assert_eq!(parsed.value, None);
    }

    #[test]
    fn lenient_u64_rejects_negative_and_out_of_range_values() {
        assert!(serde_json::from_str::<Lenient>(r#"{"value": -1}"#).is_err());
        assert!(serde_json::from_str::<Lenient>(r#"{"value": -1.5}"#).is_err());
        assert!(serde_json::from_str::<Lenient>(r#"{"value": 1e30}"#).is_err());
    }

    #[test]
    fn transcript_usage_accepts_float_written_token_counts() {
        let usage: TranscriptUsage =
            serde_json::from_str(r#"{"inputTokens": 100.0, "cost": 0.5}"#).unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cost, Some(0.5));
    }
}
