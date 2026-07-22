use serde::{Deserialize, Deserializer};
use url::Url;

pub fn non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(serde::de::Error::custom("must not be empty"))
    } else {
        Ok(value)
    }
}

pub fn optional_non_empty<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        if value.is_empty() {
            Err(serde::de::Error::custom("must not be empty"))
        } else {
            Ok(Some(value))
        }
    })
}

pub fn positive_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        Err(serde::de::Error::custom("must be at least 1"))
    } else {
        Ok(value)
    }
}

pub fn absolute_url<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Url::parse(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

pub fn optional_non_empty_vec<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Option::<Vec<String>>::deserialize(deserializer)?;
    if values
        .as_ref()
        .is_some_and(|values| values.iter().any(String::is_empty))
    {
        Err(serde::de::Error::custom("array items must not be empty"))
    } else {
        Ok(values)
    }
}

pub fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub fn optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub fn literal_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match bool::deserialize(deserializer)? {
        true => Ok(true),
        false => Err(serde::de::Error::custom("must be true")),
    }
}

pub fn literal_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match bool::deserialize(deserializer)? {
        false => Ok(false),
        true => Err(serde::de::Error::custom("must be false")),
    }
}
