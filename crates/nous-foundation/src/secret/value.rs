//! Secret values expose bytes only at an explicit consuming boundary.
use nous_domain::{Error, Result};

#[derive(Clone)]
pub struct SecretValue(pub(super) String);
impl SecretValue {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let value = value.trim();
        if value.is_empty()
            || value.len() > 4096
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(Error::invalid(
                "secret must contain one value of 1..4096 bytes",
            ));
        }
        Ok(Self(value.to_owned()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}
impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretValue([redacted])")
    }
}
