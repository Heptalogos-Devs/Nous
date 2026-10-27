//! Currency-bearing amounts and provider-independent model accounting.
use crate::{Error, Result};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{borrow::Cow, fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecimalAmount(String);
impl DecimalAmount {
    pub fn new(value: Decimal) -> Result<Self> {
        if value.is_sign_negative() && !value.is_zero() {
            return Err(Error::invalid("monetary amounts must be nonnegative"));
        }
        Ok(Self(value.normalize().to_string()))
    }
    pub fn decimal(&self) -> Decimal {
        Decimal::from_str_exact(&self.0).expect("validated decimal amount")
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl FromStr for DecimalAmount {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        if value.is_empty()
            || value.len() > 64
            || value.starts_with('.')
            || value.ends_with('.')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            return Err(Error::invalid(
                "amount must be a nonnegative decimal string",
            ));
        }
        let decimal = Decimal::from_str_exact(value)
            .map_err(|_| Error::invalid("amount exceeds supported decimal precision"))?;
        Self::new(decimal)
    }
}
impl fmt::Display for DecimalAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl Serialize for DecimalAmount {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for DecimalAmount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for DecimalAmount {
    fn schema_name() -> Cow<'static, str> {
        "DecimalAmount".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[0-9]+(\\.[0-9]+)?$"})
            .try_into().expect("decimal schema is an object")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Money {
    pub currency: String,
    pub amount: DecimalAmount,
}
impl Money {
    pub fn new(currency: String, amount: DecimalAmount) -> Result<Self> {
        validate_currency(&currency)?;
        Ok(Self { currency, amount })
    }
}
pub fn validate_currency(currency: &str) -> Result<()> {
    if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(Error::invalid(
            "currency must be an explicit three-letter uppercase code",
        ));
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenPricing {
    pub currency: String,
    pub input_per_million: DecimalAmount,
    pub output_per_million: DecimalAmount,
}
impl TokenPricing {
    pub fn validate(&self) -> Result<()> {
        validate_currency(&self.currency)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPriceSnapshot {
    pub resource_id: String,
    pub provider_id: String,
    pub request_model: String,
    pub config_digest: String,
    pub pricing: TokenPricing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    Chat,
    Embedding,
    Rerank,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    Provider,
    SdkNormalized,
    Mixed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageField {
    UsageEnvelope,
    InputTokens,
    OutputTokens,
    TotalTokens,
    CachedInputTokens,
    CacheCreationInputTokens,
    #[serde(rename = "cache_creation_5m_input_tokens")]
    CacheCreation5mInputTokens,
    #[serde(rename = "cache_creation_1h_input_tokens")]
    CacheCreation1hInputTokens,
    AcceptedPredictionOutputTokens,
    RejectedPredictionOutputTokens,
    ReasoningOutputTokens,
    AudioInputTokens,
    AudioOutputTokens,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelUsage {
    pub kind: UsageKind,
    pub source: UsageSource,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_creation_5m_input_tokens: Option<u64>,
    pub cache_creation_1h_input_tokens: Option<u64>,
    pub accepted_prediction_output_tokens: Option<u64>,
    pub rejected_prediction_output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub audio_input_tokens: Option<u64>,
    pub audio_output_tokens: Option<u64>,
    pub invalid_fields: Vec<UsageField>,
}
impl ModelUsage {
    pub fn known_total_tokens(&self) -> Option<u64> {
        self.total_tokens.or_else(|| {
            let input = self.input_tokens?;
            match self.output_tokens {
                Some(output) => input.checked_add(output),
                None if matches!(self.kind, UsageKind::Embedding | UsageKind::Rerank) => {
                    Some(input)
                }
                _ => None,
            }
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostMissing {
    Pricing,
    UsageEnvelope,
    InputTokens,
    OutputTokens,
    ProviderCurrency,
    InvalidProviderCost,
    ArithmeticOverflow,
    ArithmeticPrecision,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelCost {
    pub pricing: Option<ModelPriceSnapshot>,
    /// Estimate under the configured flat rates per reported input/output token.
    pub estimated: Option<Money>,
    /// Only a provider amount with an explicit currency is a reported charge.
    pub reported: Option<Money>,
    pub missing: Vec<CostMissing>,
    /// Original provider billing value and its explicit currency, separate from normalized money.
    pub reported_evidence: Option<serde_json::Value>,
}
