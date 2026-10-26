use super::{ModelFailure, RetryClass};
use nous_domain::{EmbeddingNormalization, EmbeddingSpace, ErrorCode};

#[derive(Clone)]
pub(super) struct ResponseContract {
    pub space: EmbeddingSpace,
    pub inputs: usize,
    pub max_bytes: usize,
}
#[derive(Clone)]
pub(super) enum PreparedEmbeddingRequest {
    Openai(super::openai_embedding::Request),
    Ark(super::ark_embedding::Request),
}

pub(super) fn validate_model(actual: &str, space: &EmbeddingSpace) -> Result<(), ModelFailure> {
    if actual != space.provider_model {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::Conflict,
            "embedding provider model differs from the configured immutable space",
        ));
    }
    Ok(())
}
pub(super) fn normalize(
    mut vector: Vec<f32>,
    space: &EmbeddingSpace,
) -> Result<Vec<f32>, ModelFailure> {
    if vector.len() != space.dimension
        || !vector.iter().all(|value| value.is_finite())
        || !vector.iter().any(|value| *value != 0.0)
    {
        return Err(invalid(
            "embedding response must contain finite nonzero vectors with the configured dimension",
        ));
    }
    if space.normalization == EmbeddingNormalization::L2 {
        let norm = vector
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
            .sqrt();
        for value in &mut vector {
            *value = (f64::from(*value) / norm) as f32;
        }
        if !vector.iter().all(|value| value.is_finite())
            || !vector.iter().any(|value| *value != 0.0)
        {
            return Err(invalid(
                "embedding normalization produced an invalid vector",
            ));
        }
    }
    Ok(vector)
}
pub(super) fn invalid(message: &str) -> ModelFailure {
    ModelFailure::new(RetryClass::Permanent, ErrorCode::InvalidArgument, message)
}
