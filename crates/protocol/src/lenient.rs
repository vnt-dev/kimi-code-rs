//! Lenient integer deserializers for wire fields that used to be JSON numbers.
//!
//! Legacy writers (and JS clients) may emit integer-valued floats such as
//! `100.0`, so these deserializers accept both JSON `100` and `100.0`.
//! Non-integer floats are truncated toward zero, mirroring
//! `ids::unsigned_id`; NaN, Infinity, and out-of-range values are rejected.

use std::fmt;
use std::marker::PhantomData;

use serde::Deserializer;
use serde::de::Visitor;

trait LenientInt: Copy {
    fn label() -> &'static str;
    fn from_u64(value: u64) -> Result<Self, String>;
    fn from_i64(value: i64) -> Result<Self, String>;
    fn from_f64(value: f64) -> Result<Self, String>;
}

fn lenient_int_from_f64(
    value: f64,
    label: &str,
    low: f64,
    high_exclusive: f64,
) -> Result<f64, String> {
    if !value.is_finite() {
        return Err(format!("{label} must be finite, got {value}"));
    }
    // Check the raw value against the lower bound before truncating: a
    // negative fraction such as -0.1 truncates toward zero to -0.0, which
    // compares equal to 0.0 and would otherwise slip through.
    if value < low {
        return Err(format!("{label} is out of range: {value}"));
    }
    let truncated = value.trunc();
    if truncated >= high_exclusive {
        return Err(format!("{label} is out of range: {value}"));
    }
    Ok(truncated)
}

impl LenientInt for u64 {
    fn label() -> &'static str {
        "u64"
    }

    fn from_u64(value: u64) -> Result<Self, String> {
        Ok(value)
    }

    fn from_i64(value: i64) -> Result<Self, String> {
        u64::try_from(value).map_err(|_| format!("u64 is out of range: {value}"))
    }

    fn from_f64(value: f64) -> Result<Self, String> {
        // 2^64, the smallest value not representable as u64.
        lenient_int_from_f64(value, "u64", 0.0, 18446744073709551616.0).map(|v| v as u64)
    }
}

impl LenientInt for i64 {
    fn label() -> &'static str {
        "i64"
    }

    fn from_u64(value: u64) -> Result<Self, String> {
        i64::try_from(value).map_err(|_| format!("i64 is out of range: {value}"))
    }

    fn from_i64(value: i64) -> Result<Self, String> {
        Ok(value)
    }

    fn from_f64(value: f64) -> Result<Self, String> {
        // (-2^63, 2^63); both bounds are exact in f64.
        lenient_int_from_f64(value, "i64", -9223372036854775808.0, 9223372036854775808.0)
            .map(|v| v as i64)
    }
}

impl LenientInt for u32 {
    fn label() -> &'static str {
        "u32"
    }

    fn from_u64(value: u64) -> Result<Self, String> {
        u32::try_from(value).map_err(|_| format!("u32 is out of range: {value}"))
    }

    fn from_i64(value: i64) -> Result<Self, String> {
        u32::try_from(value).map_err(|_| format!("u32 is out of range: {value}"))
    }

    fn from_f64(value: f64) -> Result<Self, String> {
        lenient_int_from_f64(value, "u32", 0.0, 4294967296.0).map(|v| v as u32)
    }
}

impl LenientInt for i32 {
    fn label() -> &'static str {
        "i32"
    }

    fn from_u64(value: u64) -> Result<Self, String> {
        i32::try_from(value).map_err(|_| format!("i32 is out of range: {value}"))
    }

    fn from_i64(value: i64) -> Result<Self, String> {
        i32::try_from(value).map_err(|_| format!("i32 is out of range: {value}"))
    }

    fn from_f64(value: f64) -> Result<Self, String> {
        lenient_int_from_f64(value, "i32", -2147483648.0, 2147483648.0).map(|v| v as i32)
    }
}

impl LenientInt for u16 {
    fn label() -> &'static str {
        "u16"
    }

    fn from_u64(value: u64) -> Result<Self, String> {
        u16::try_from(value).map_err(|_| format!("u16 is out of range: {value}"))
    }

    fn from_i64(value: i64) -> Result<Self, String> {
        u16::try_from(value).map_err(|_| format!("u16 is out of range: {value}"))
    }

    fn from_f64(value: f64) -> Result<Self, String> {
        lenient_int_from_f64(value, "u16", 0.0, 65536.0).map(|v| v as u16)
    }
}

struct LenientIntVisitor<T>(PhantomData<T>);

impl<T> Visitor<'_> for LenientIntVisitor<T>
where
    T: LenientInt,
{
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "an integer or finite number fitting {}",
            T::label()
        )
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        T::from_u64(value).map_err(E::custom)
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        T::from_i64(value).map_err(E::custom)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        T::from_f64(value).map_err(E::custom)
    }
}

struct LenientNullableVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for LenientNullableVisitor<T>
where
    T: LenientInt,
{
    type Value = Option<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "null, an integer, or a finite number fitting {}",
            T::label()
        )
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer
            .deserialize_any(LenientIntVisitor::<T>(PhantomData))
            .map(Some)
    }
}

macro_rules! lenient_int_deserializers {
    ($ty:ty, $plain:ident, $nullable:ident, $optional:ident) => {
        /// Deserializes a required integer field, accepting `100` and `100.0`.
        pub fn $plain<'de, D>(deserializer: D) -> Result<$ty, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(LenientIntVisitor::<$ty>(PhantomData))
        }

        /// Deserializes a present-but-nullable integer field (`null`, `100`,
        /// or `100.0`). Pair with `#[serde(default)]` to also allow a missing
        /// key.
        pub fn $nullable<'de, D>(deserializer: D) -> Result<Option<$ty>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_option(LenientNullableVisitor::<$ty>(PhantomData))
        }

        /// Deserializes an optional non-null integer field; use in
        /// `#[serde(default, deserialize_with = ...)]` slots where a present
        /// value is never `null`.
        pub fn $optional<'de, D>(deserializer: D) -> Result<Option<$ty>, D::Error>
        where
            D: Deserializer<'de>,
        {
            $plain(deserializer).map(Some)
        }
    };
}

lenient_int_deserializers!(u64, lenient_u64, lenient_nullable_u64, lenient_optional_u64);
lenient_int_deserializers!(i64, lenient_i64, lenient_nullable_i64, lenient_optional_i64);
lenient_int_deserializers!(u32, lenient_u32, lenient_nullable_u32, lenient_optional_u32);
lenient_int_deserializers!(i32, lenient_i32, lenient_nullable_i32, lenient_optional_i32);
lenient_int_deserializers!(u16, lenient_u16, lenient_nullable_u16, lenient_optional_u16);

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct Plain {
        #[serde(deserialize_with = "lenient_u64")]
        value: u64,
    }

    #[derive(Debug, Deserialize)]
    struct Nullable {
        #[serde(default, deserialize_with = "lenient_nullable_i64")]
        value: Option<i64>,
    }

    #[test]
    fn accepts_integers_and_integer_valued_floats() {
        let parsed: Plain = serde_json::from_str(r#"{"value": 100}"#).unwrap();
        assert_eq!(parsed.value, 100);
        let parsed: Plain = serde_json::from_str(r#"{"value": 100.0}"#).unwrap();
        assert_eq!(parsed.value, 100);
        let parsed: Plain = serde_json::from_str(r#"{"value": 100.9}"#).unwrap();
        assert_eq!(parsed.value, 100);
    }

    #[test]
    fn rejects_non_finite_and_out_of_range() {
        assert!(serde_json::from_str::<Plain>(r#"{"value": 1e30}"#).is_err());
        assert!(serde_json::from_str::<Plain>(r#"{"value": -1}"#).is_err());
        assert!(serde_json::from_str::<Plain>(r#"{"value": -0.1}"#).is_err());
        assert!(serde_json::from_str::<Plain>(r#"{"value": "100"}"#).is_err());
    }

    #[test]
    fn nullable_accepts_null_missing_and_float() {
        let parsed: Nullable = serde_json::from_str(r#"{"value": null}"#).unwrap();
        assert_eq!(parsed.value, None);
        let parsed: Nullable = serde_json::from_str(r"{}").unwrap();
        assert_eq!(parsed.value, None);
        let parsed: Nullable = serde_json::from_str(r#"{"value": -3.0}"#).unwrap();
        assert_eq!(parsed.value, Some(-3));
    }
}
