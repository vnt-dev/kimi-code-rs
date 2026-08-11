use std::fmt;

use serde::{Deserializer, de::Visitor};

macro_rules! define_unsigned_id {
    ($(#[$meta:meta])* $name:ident, $label:literal, $accept_prefixed_string:expr) => {
        $(#[$meta])*
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const MAX: Self = Self(u64::MAX);

            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl std::ops::Add<u64> for $name {
            type Output = Self;

            fn add(self, rhs: u64) -> Self::Output {
                Self(self.0 + rhs)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl TryFrom<i64> for $name {
            type Error = std::num::TryFromIntError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                u64::try_from(value).map(Self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                super::unsigned_id::deserialize_unsigned_id(
                    deserializer,
                    $label,
                    $accept_prefixed_string,
                )
                .map(Self)
            }
        }
    };
}

pub(crate) use define_unsigned_id;

pub(super) fn deserialize_unsigned_id<'de, D>(
    deserializer: D,
    label: &'static str,
    accept_prefixed_string: bool,
) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(UnsignedIdVisitor {
        label,
        accept_prefixed_string,
    })
}

struct UnsignedIdVisitor {
    label: &'static str,
    accept_prefixed_string: bool,
}

impl Visitor<'_> for UnsignedIdVisitor {
    type Value = u64;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a non-negative integer, finite number, or numeric {} string",
            self.label
        )
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(value)
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        u64::try_from(value).map_err(E::custom)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        unsigned_id_from_f64(value, self.label).map_err(E::custom)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        parse_unsigned_id(value, self.label, self.accept_prefixed_string).map_err(E::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value)
    }
}

fn unsigned_id_from_f64(value: f64, label: &str) -> Result<u64, String> {
    if !value.is_finite() {
        return Err(format!("{label} must be finite, got {value}"));
    }
    let truncated = value.trunc();
    let upper_exclusive = u64::MAX as f64;
    if truncated < 0.0 || truncated >= upper_exclusive {
        return Err(format!(
            "{label} is outside the unsigned 64-bit range: {value}"
        ));
    }
    Ok(truncated as u64)
}

fn parse_unsigned_id(
    value: &str,
    label: &str,
    accept_prefixed_string: bool,
) -> Result<u64, String> {
    if value.starts_with('-') {
        return Err(format!("invalid {label} {value:?}"));
    }
    parse_numeric_unsigned_id(value, label)
        .or_else(|_| {
            if !accept_prefixed_string {
                return Err(());
            }
            let without_first = value
                .char_indices()
                .nth(1)
                .map_or("", |(index, _)| &value[index..]);
            parse_numeric_unsigned_id(without_first, label)
        })
        .map_err(|_| format!("invalid {label} {value:?}"))
}

fn parse_numeric_unsigned_id(value: &str, label: &str) -> Result<u64, ()> {
    if let Ok(value) = value.parse::<u64>() {
        return Ok(value);
    }
    value
        .parse::<f64>()
        .map_err(|_| ())
        .and_then(|value| unsigned_id_from_f64(value, label).map_err(|_| ()))
}
