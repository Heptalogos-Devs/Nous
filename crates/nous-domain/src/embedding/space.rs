//! Semantic space identity is independent from the route that produced a vector.
use crate::{Error, Result, validate_name};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingNormalization {
    #[default]
    None,
    L2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingSpace {
    pub id: String,
    pub provider_model: String,
    pub dimension: usize,
    pub normalization: EmbeddingNormalization,
    pub parameters: BTreeMap<String, Value>,
}
impl EmbeddingSpace {
    pub fn new(
        provider_model: String,
        dimension: usize,
        normalization: EmbeddingNormalization,
        parameters: BTreeMap<String, Value>,
    ) -> Result<Self> {
        let mut value = Self {
            id: String::new(),
            provider_model,
            dimension,
            normalization,
            parameters,
        };
        value.id = value.computed_id()?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.provider_model, "embedding provider model")?;
        if self.dimension == 0 || self.dimension > 65536 || self.id != self.computed_id()? {
            return Err(Error::invalid(
                "embedding space identity or dimension is invalid",
            ));
        }
        Ok(())
    }
    fn computed_id(&self) -> Result<String> {
        hash(
            "embspace",
            json!({"provider_model":self.provider_model,"dimension":self.dimension,"normalization":self.normalization,"parameters":self.parameters}),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingProducer {
    pub signature: String,
    pub space_id: String,
    pub resource_id: String,
    pub provider: String,
    pub protocol: String,
    pub endpoint: String,
    pub request_model: String,
}
impl EmbeddingProducer {
    pub fn new(
        space_id: String,
        resource_id: String,
        provider: String,
        protocol: String,
        endpoint: String,
        request_model: String,
    ) -> Result<Self> {
        let mut value = Self {
            signature: String::new(),
            space_id,
            resource_id,
            provider,
            protocol,
            endpoint,
            request_model,
        };
        value.signature = value.computed_signature()?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        for field in [
            &self.space_id,
            &self.resource_id,
            &self.provider,
            &self.protocol,
            &self.request_model,
        ] {
            validate_name(field, "embedding producer field")?;
        }
        if self.endpoint.is_empty()
            || self.endpoint.len() > 4096
            || self.endpoint.contains('\0')
            || self.signature != self.computed_signature()?
        {
            return Err(Error::invalid(
                "embedding producer identity or endpoint is invalid",
            ));
        }
        Ok(())
    }
    fn computed_signature(&self) -> Result<String> {
        hash(
            "embproducer",
            json!({"space_id":self.space_id,"resource_id":self.resource_id,"provider":self.provider,"protocol":self.protocol,"endpoint":self.endpoint,"request_model":self.request_model}),
        )
    }
}
fn hash(prefix: &str, value: Value) -> Result<String> {
    Ok(format!(
        "{prefix}_{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(value))?)
    ))
}
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        value => value,
    }
}
