use nous_domain::Result;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
pub(crate) fn enum_name<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .expect("unit enum name")
        .to_owned())
}
pub(crate) fn json_option<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    Ok(value.as_ref().map(serde_json::to_string).transpose()?)
}
pub(crate) fn parse_enum<T: DeserializeOwned>(value: &str) -> Result<T> {
    Ok(serde_json::from_value(Value::String(value.into()))?)
}
pub(crate) fn parse_optional<T: DeserializeOwned>(value: Option<&str>) -> Result<Option<T>> {
    Ok(value.map(serde_json::from_str).transpose()?)
}
