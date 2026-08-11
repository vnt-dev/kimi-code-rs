//! Canonical numeric identifier for a step within an agent-loop turn.

use std::{fmt, ops::Add};

use serde::{Deserialize, Deserializer, Serialize, de::Visitor};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct StepId(u64);

impl StepId {
    pub const MAX: Self = Self(u64::MAX);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Add<u64> for StepId {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl fmt::Display for StepId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<u64> for StepId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<StepId> for u64 {
    fn from(value: StepId) -> Self {
        value.0
    }
}

impl TryFrom<i64> for StepId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value).map(Self)
    }
}

impl<'de> Deserialize<'de> for StepId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StepIdVisitor)
    }
}

struct StepIdVisitor;

impl Visitor<'_> for StepIdVisitor {
    type Value = StepId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a non-negative integer, finite number, or numeric step id string")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StepId::new(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        StepId::try_from(value).map_err(E::custom)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        step_id_from_f64(value).map_err(E::custom)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        parse_step_id(value).map_err(E::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value)
    }
}

fn step_id_from_f64(value: f64) -> Result<StepId, String> {
    if !value.is_finite() {
        return Err(format!("step id must be finite, got {value}"));
    }
    let truncated = value.trunc();
    let upper_exclusive = u64::MAX as f64;
    if truncated < 0.0 || truncated >= upper_exclusive {
        return Err(format!(
            "step id is outside the unsigned 64-bit range: {value}"
        ));
    }
    Ok(StepId::new(truncated as u64))
}

fn parse_step_id(value: &str) -> Result<StepId, String> {
    if let Ok(value) = value.parse::<u64>() {
        return Ok(StepId::new(value));
    }
    value
        .parse::<f64>()
        .map_err(|_| format!("invalid step id {value:?}"))
        .and_then(step_id_from_f64)
}

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
