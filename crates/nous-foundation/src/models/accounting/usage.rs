use genai::chat::Usage;
use nous_domain::{ModelUsage, UsageField, UsageKind, UsageSource};
use serde_json::Value;

pub fn normalize_usage(
    kind: UsageKind,
    sdk: Option<&Usage>,
    body: Option<&Value>,
) -> Option<ModelUsage> {
    let raw = body.and_then(|body| body.get("usage"));
    let mut normalization = Normalizer {
        raw,
        provider: false,
        sdk: false,
        invalid: Vec::new(),
    };
    if raw.is_some_and(|value| !value.is_null() && !value.is_object()) {
        normalization.invalid.push(UsageField::UsageEnvelope);
        normalization.provider = true;
    }
    let prompt = sdk.and_then(|usage| usage.prompt_tokens_details.as_ref());
    let completion = sdk.and_then(|usage| usage.completion_tokens_details.as_ref());
    let input = normalization.count(
        UsageField::InputTokens,
        &["/prompt_tokens", "/input_tokens"],
        sdk.and_then(|usage| usage.prompt_tokens),
    );
    let output = normalization.count(
        UsageField::OutputTokens,
        &["/completion_tokens", "/output_tokens"],
        sdk.and_then(|usage| usage.completion_tokens),
    );
    let total = normalization.count(
        UsageField::TotalTokens,
        &["/total_tokens"],
        sdk.and_then(|usage| usage.total_tokens),
    );
    let cached = normalization.count(
        UsageField::CachedInputTokens,
        &[
            "/prompt_tokens_details/cached_tokens",
            "/input_tokens_details/cached_tokens",
            "/cache_read_input_tokens",
        ],
        prompt.and_then(|p| p.cached_tokens),
    );
    let created = normalization.count(
        UsageField::CacheCreationInputTokens,
        &[
            "/prompt_tokens_details/cache_creation_tokens",
            "/prompt_tokens_details/cache_write_tokens",
            "/cache_creation_input_tokens",
        ],
        prompt.and_then(|p| p.cache_creation_tokens),
    );
    let cache = prompt.and_then(|prompt| prompt.cache_creation_details.as_ref());
    let cache_5m = normalization.count(
        UsageField::CacheCreation5mInputTokens,
        &[
            "/prompt_tokens_details/cache_creation_details/ephemeral_5m_tokens",
            "/cache_creation/ephemeral_5m_input_tokens",
        ],
        cache.and_then(|cache| cache.ephemeral_5m_tokens),
    );
    let cache_1h = normalization.count(
        UsageField::CacheCreation1hInputTokens,
        &[
            "/prompt_tokens_details/cache_creation_details/ephemeral_1h_tokens",
            "/cache_creation/ephemeral_1h_input_tokens",
        ],
        cache.and_then(|cache| cache.ephemeral_1h_tokens),
    );
    let accepted = normalization.count(
        UsageField::AcceptedPredictionOutputTokens,
        &[
            "/completion_tokens_details/accepted_prediction_tokens",
            "/output_tokens_details/accepted_prediction_tokens",
        ],
        completion.and_then(|completion| completion.accepted_prediction_tokens),
    );
    let rejected = normalization.count(
        UsageField::RejectedPredictionOutputTokens,
        &[
            "/completion_tokens_details/rejected_prediction_tokens",
            "/output_tokens_details/rejected_prediction_tokens",
        ],
        completion.and_then(|completion| completion.rejected_prediction_tokens),
    );
    let reasoning = normalization.count(
        UsageField::ReasoningOutputTokens,
        &[
            "/completion_tokens_details/reasoning_tokens",
            "/output_tokens_details/reasoning_tokens",
        ],
        completion.and_then(|p| p.reasoning_tokens),
    );
    let audio_input = normalization.count(
        UsageField::AudioInputTokens,
        &[
            "/prompt_tokens_details/audio_tokens",
            "/input_tokens_details/audio_tokens",
        ],
        prompt.and_then(|p| p.audio_tokens),
    );
    let audio_output = normalization.count(
        UsageField::AudioOutputTokens,
        &[
            "/completion_tokens_details/audio_tokens",
            "/output_tokens_details/audio_tokens",
        ],
        completion.and_then(|p| p.audio_tokens),
    );
    if [
        input,
        output,
        total,
        cached,
        created,
        reasoning,
        audio_input,
        audio_output,
        cache_5m,
        cache_1h,
        accepted,
        rejected,
    ]
    .iter()
    .all(Option::is_none)
        && normalization.invalid.is_empty()
    {
        return None;
    }
    let source = match (normalization.provider, normalization.sdk) {
        (true, true) => UsageSource::Mixed,
        (true, false) => UsageSource::Provider,
        _ => UsageSource::SdkNormalized,
    };
    Some(ModelUsage {
        kind,
        source,
        input_tokens: input,
        output_tokens: output,
        total_tokens: total,
        cached_input_tokens: cached,
        cache_creation_input_tokens: created,
        cache_creation_5m_input_tokens: cache_5m,
        cache_creation_1h_input_tokens: cache_1h,
        accepted_prediction_output_tokens: accepted,
        rejected_prediction_output_tokens: rejected,
        reasoning_output_tokens: reasoning,
        audio_input_tokens: audio_input,
        audio_output_tokens: audio_output,
        invalid_fields: normalization.invalid,
    })
}
struct Normalizer<'a> {
    raw: Option<&'a Value>,
    provider: bool,
    sdk: bool,
    invalid: Vec<UsageField>,
}
impl Normalizer<'_> {
    fn count(&mut self, field: UsageField, paths: &[&str], sdk: Option<i32>) -> Option<u64> {
        if let Some(value) = self
            .raw
            .and_then(|raw| paths.iter().find_map(|path| raw.pointer(path)))
            .filter(|value| !value.is_null())
        {
            self.provider = true;
            if let Some(count) = value.as_u64().or_else(|| {
                value
                    .as_f64()
                    .filter(|count| {
                        count.is_finite()
                            && *count >= 0.0
                            && count.fract() == 0.0
                            && *count <= 9_007_199_254_740_991.0
                    })
                    .map(|count| count as u64)
            }) {
                return Some(count);
            }
            self.invalid.push(field);
            return None;
        }
        if let Some(value) = sdk {
            self.sdk = true;
            if let Ok(count) = u64::try_from(value) {
                return Some(count);
            }
            self.invalid.push(field);
        }
        None
    }
}
