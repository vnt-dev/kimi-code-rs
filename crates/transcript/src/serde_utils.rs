use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
