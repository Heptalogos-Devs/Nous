//! One UUID representation and validation rule for all domain identities.
use crate::{Error, Result};

pub(crate) fn parse_uuid(value: &str) -> Result<uuid::Uuid> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| Error::invalid("identity must be a UUID"))?;
    if id.is_nil() || id.to_string() != value {
        return Err(Error::invalid("identity must be a canonical nonnil UUID"));
    }
    Ok(id)
}

pub(crate) fn validate_uuid(value: &str) -> Result<()> {
    parse_uuid(value).map(|_| ())
}
