//! Identifier vocabulary for the transcript model.
//!
//! Original: `packages/transcript/src/model/ids.ts`.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

string_id!(TurnId);
string_id!(StepId);
string_id!(FrameId);
string_id!(MarkerId);
string_id!(TaskRefId);
string_id!(TaskId);
string_id!(AgentId);
string_id!(InteractionId);
string_id!(AttachmentId);
string_id!(TodoId);
string_id!(ItemId);

pub fn turn_id(ordinal: i64) -> TurnId {
    TurnId::new(format!("t{ordinal}"))
}

pub fn step_id(turn: &TurnId, ordinal: i64) -> StepId {
    StepId::new(format!("{turn}.{ordinal}"))
}

pub fn frame_id(step: &StepId, ordinal: i64) -> FrameId {
    FrameId::new(format!("{step}.f{ordinal}"))
}

/// Extract the numeric portion of a turn id, matching the useful finite
/// subset of JavaScript's `Number(id.slice(1))`. Invalid and non-finite
/// values become zero.
pub fn turn_ordinal(id: &TurnId) -> f64 {
    let value = id.as_ref().get(1..).unwrap_or_default().trim();
    if value.is_empty() {
        return 0.0;
    }

    let parsed = parse_prefixed_integer(value)
        .map(|value| value as f64)
        .or_else(|| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    if parsed.is_finite() { parsed } else { 0.0 }
}

/// Compare turn ids by the ordinal embedded after their leading character.
pub fn compare_turn_ids(a: &TurnId, b: &TurnId) -> f64 {
    turn_ordinal(a) - turn_ordinal(b)
}

fn parse_prefixed_integer(value: &str) -> Option<i128> {
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |rest| (true, rest));
    let (radix, digits) = if let Some(digits) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        (16, digits)
    } else if let Some(digits) = unsigned
        .strip_prefix("0b")
        .or_else(|| unsigned.strip_prefix("0B"))
    {
        (2, digits)
    } else if let Some(digits) = unsigned
        .strip_prefix("0o")
        .or_else(|| unsigned.strip_prefix("0O"))
    {
        (8, digits)
    } else {
        return None;
    };

    i128::from_str_radix(digits, radix).ok().and_then(|number| {
        if negative {
            number.checked_neg()
        } else {
            Some(number)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_and_compares_hierarchical_ids() {
        let turn = turn_id(10);
        let step = step_id(&turn, 2);
        assert_eq!(turn.as_ref(), "t10");
        assert_eq!(step.as_ref(), "t10.2");
        assert_eq!(frame_id(&step, 4).as_ref(), "t10.2.f4");
        assert!(compare_turn_ids(&TurnId::from("t2"), &turn) < 0.0);
        assert_eq!(turn_ordinal(&TurnId::from("bad")), 0.0);
    }
}
