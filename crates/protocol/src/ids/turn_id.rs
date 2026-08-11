//! Canonical identifier for an agent-loop turn.
//!
//! Runtime turn identifiers are integral counters. The newtype keeps that
//! invariant visible at API boundaries without changing their JSON number
//! representation. Transcript identifiers (for example, `"t7"`) remain a
//! separate type because they belong to the transcript model.

use std::{fmt, ops::Add};

use serde::{Deserialize, Deserializer, Serialize, de::Visitor};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TurnId(i64);

impl TurnId {
    pub const MAX: Self = Self(i64::MAX);

    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl Add<i64> for TurnId {
    type Output = Self;

    fn add(self, rhs: i64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<i64> for TurnId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<TurnId> for i64 {
    fn from(value: TurnId) -> Self {
        value.0
    }
}

impl TryFrom<u64> for TurnId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value).map(Self)
    }
}

impl<'de> Deserialize<'de> for TurnId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(TurnIdVisitor)
    }
}

struct TurnIdVisitor;

impl Visitor<'_> for TurnIdVisitor {
    type Value = TurnId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an integer, a finite number, or a numeric turn id string")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(TurnId::new(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        TurnId::try_from(value).map_err(E::custom)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        turn_id_from_f64(value).map_err(E::custom)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        parse_turn_id(value).map_err(E::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value)
    }
}

fn turn_id_from_f64(value: f64) -> Result<TurnId, String> {
    if !value.is_finite() {
        return Err(format!("turn id must be finite, got {value}"));
    }
    let truncated = value.trunc();
    let upper_exclusive = -(i64::MIN as f64);
    if truncated < i64::MIN as f64 || truncated >= upper_exclusive {
        return Err(format!(
            "turn id is outside the signed 64-bit range: {value}"
        ));
    }
    Ok(TurnId::new(truncated as i64))
}

fn parse_turn_id(value: &str) -> Result<TurnId, String> {
    parse_numeric_turn_id(value)
        .or_else(|_| {
            let without_first = value
                .char_indices()
                .nth(1)
                .map_or("", |(index, _)| &value[index..]);
            parse_numeric_turn_id(without_first)
        })
        .map_err(|_| format!("invalid turn id {value:?}"))
}

fn parse_numeric_turn_id(value: &str) -> Result<TurnId, ()> {
    if let Ok(value) = value.parse::<i64>() {
        return Ok(TurnId::new(value));
    }
    value
        .parse::<f64>()
        .map_err(|_| ())
        .and_then(|value| turn_id_from_f64(value).map_err(|_| ()))
}

struct NonNegativeTurnId(TurnId);

impl<'de> Deserialize<'de> for NonNegativeTurnId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NonNegativeTurnIdVisitor)
    }
}

struct NonNegativeTurnIdVisitor;

impl Visitor<'_> for NonNegativeTurnIdVisitor {
    type Value = NonNegativeTurnId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a non-negative integer turn id")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        TurnId::try_from(value)
            .map(NonNegativeTurnId)
            .map_err(E::custom)
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value >= 0 {
            Ok(NonNegativeTurnId(TurnId::new(value)))
        } else {
            Err(E::custom("turn id must be non-negative"))
        }
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value < -(i64::MIN as f64) {
            Ok(NonNegativeTurnId(TurnId::new(value as i64)))
        } else {
            Err(E::custom("turn id must be a non-negative integer"))
        }
    }
}

/// Serde adapter for protocol fields that require a non-negative turn id.
pub mod non_negative {
    use serde::{Deserialize, Deserializer};

    use super::{NonNegativeTurnId, TurnId};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<TurnId, D::Error>
    where
        D: Deserializer<'de>,
    {
        NonNegativeTurnId::deserialize(deserializer).map(|value| value.0)
    }
}

/// Serde adapter for optional protocol fields that require a non-negative turn id.
pub mod non_negative_option {
    use serde::{Deserialize, Deserializer};

    use super::{NonNegativeTurnId, TurnId};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<TurnId>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<NonNegativeTurnId>::deserialize(deserializer)
            .map(|value| value.map(|value| value.0))
    }
}

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

    #[derive(Debug, Deserialize, PartialEq)]
    struct NonNegativeRecord {
        #[serde(deserialize_with = "non_negative::deserialize")]
        turn_id: TurnId,
        #[serde(default, deserialize_with = "non_negative_option::deserialize")]
        optional_turn_id: Option<TurnId>,
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
    fn deserializes_integer_float_and_prefixed_string_representations() {
        for (value, expected) in [
            (json!(7), 7),
            (json!(7.9), 7),
            (json!(-7.9), -7),
            (json!("100"), 100),
            (json!("t100"), 100),
            (json!("t100.9"), 100),
        ] {
            assert_eq!(
                serde_json::from_value::<TurnId>(value).unwrap(),
                TurnId::new(expected)
            );
        }
        assert!(serde_json::from_value::<TurnId>(json!("turn-1")).is_err());
    }

    #[test]
    fn non_negative_adapters_preserve_rest_validation() {
        assert_eq!(
            serde_json::from_value::<NonNegativeRecord>(json!({
                "turn_id": 7,
                "optional_turn_id": 8
            }))
            .unwrap(),
            NonNegativeRecord {
                turn_id: TurnId::new(7),
                optional_turn_id: Some(TurnId::new(8))
            }
        );
        assert!(serde_json::from_value::<NonNegativeRecord>(json!({"turn_id": -1})).is_err());
        assert!(serde_json::from_value::<NonNegativeRecord>(json!({"turn_id": 1.5})).is_err());
        assert!(serde_json::from_value::<NonNegativeRecord>(json!({"turn_id": "1"})).is_err());
        assert!(
            serde_json::from_value::<NonNegativeRecord>(json!({
                "turn_id": 1,
                "optional_turn_id": -1
            }))
            .is_err()
        );
    }
}
