//! Serde helpers for timestamp fields that are transitioning from RFC3339
//! strings to native SurrealDB datetime values.

use chrono::{DateTime, Utc};
use serde::de::Error;
use serde::{Deserialize, Deserializer};

/// Deserialize a timestamp into `DateTime<Utc>`.
///
/// Accepts normal RFC3339 strings and tolerant object shapes by recursively
/// searching for an RFC3339 string. This keeps reads compatible while SurrealDB
/// rows transition from string fields to native datetime fields.
pub fn deserialize_datetime<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    parse_datetime_value(&value).map_err(D::Error::custom)
}

/// Deserialize an optional timestamp into `DateTime<Utc>`.
pub fn deserialize_optional_datetime<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    value
        .as_ref()
        .map(parse_datetime_value)
        .transpose()
        .map_err(D::Error::custom)
}

/// Deserialize a timestamp into a normalized RFC3339 string.
pub fn deserialize_rfc3339_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    parse_datetime_value(&value)
        .map(|dt| dt.to_rfc3339())
        .map_err(D::Error::custom)
}

/// Deserialize an optional timestamp into a normalized RFC3339 string.
pub fn deserialize_optional_rfc3339_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    value
        .as_ref()
        .map(parse_datetime_value)
        .transpose()
        .map(|dt| dt.map(|dt| dt.to_rfc3339()))
        .map_err(D::Error::custom)
}

fn parse_datetime_value(value: &serde_json::Value) -> Result<DateTime<Utc>, String> {
    match value {
        serde_json::Value::String(s) => parse_datetime_str(s),
        serde_json::Value::Object(map) => {
            for key in ["datetime", "Datetime", "$date", "date", "value"] {
                if let Some(value) = map.get(key) {
                    if let Ok(dt) = parse_datetime_value(value) {
                        return Ok(dt);
                    }
                }
            }
            for value in map.values() {
                if let Ok(dt) = parse_datetime_value(value) {
                    return Ok(dt);
                }
            }
            Err(format!(
                "could not find RFC3339 timestamp in object: {value}"
            ))
        }
        _ => Err(format!("unsupported timestamp value: {value}")),
    }
}

fn parse_datetime_str(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("invalid RFC3339 timestamp '{value}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Wrapper {
        #[serde(deserialize_with = "deserialize_datetime")]
        at: DateTime<Utc>,
    }

    #[derive(Debug, Deserialize)]
    struct StringWrapper {
        #[serde(deserialize_with = "deserialize_rfc3339_string")]
        at: String,
    }

    #[test]
    fn deserializes_rfc3339_string() {
        let parsed: Wrapper = serde_json::from_str(r#"{"at":"2026-04-26T10:00:00Z"}"#).unwrap();
        assert_eq!(parsed.at.to_rfc3339(), "2026-04-26T10:00:00+00:00");
    }

    #[test]
    fn deserializes_nested_datetime_shape() {
        let parsed: Wrapper =
            serde_json::from_str(r#"{"at":{"Datetime":"2026-04-26T10:00:00Z"}}"#).unwrap();
        assert_eq!(parsed.at.to_rfc3339(), "2026-04-26T10:00:00+00:00");
    }

    #[test]
    fn normalizes_to_rfc3339_string() {
        let parsed: StringWrapper =
            serde_json::from_str(r#"{"at":{"datetime":"2026-04-26T10:00:00Z"}}"#).unwrap();
        assert_eq!(parsed.at, "2026-04-26T10:00:00+00:00");
    }
}
