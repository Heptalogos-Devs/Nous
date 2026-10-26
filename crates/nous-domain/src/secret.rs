//! References identify protected credential files; secret bytes stay with their owner.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Component, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, JsonSchema)]
pub struct SecretRef {
    pub file: PathBuf,
}

impl SecretRef {
    pub fn canonical_path(&self) -> Result<String> {
        self.validate()?;
        self.file
            .components()
            .map(|part| match part {
                Component::Normal(part) => part
                    .to_str()
                    .filter(|part| !part.contains('\\'))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        Error::invalid("secret references must use portable UTF-8 path components")
                    }),
                _ => Err(Error::invalid(
                    "secret reference has a non-normal path component",
                )),
            })
            .collect::<Result<Vec<_>>>()
            .map(|parts| parts.join("/"))
    }
    pub fn validate(&self) -> Result<()> {
        if self.file.is_absolute()
            || !self.file.starts_with("credentials")
            || self.file.components().count() < 2
            || self
                .file
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(Error::invalid(
                "secret references must name a relative file inside credentials",
            ));
        }
        Ok(())
    }
}

impl Serialize for SecretRef {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let path = self
            .canonical_path()
            .map_err(<S::Error as serde::ser::Error>::custom)?;
        let mut output = serializer.serialize_struct("SecretRef", 1)?;
        output.serialize_field("file", &path)?;
        output.end()
    }
}
impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            file: PathBuf,
        }
        let input = Input::deserialize(deserializer)?;
        let reference = Self { file: input.file };
        let canonical = reference
            .canonical_path()
            .map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(Self {
            file: PathBuf::from(canonical),
        })
    }
}
