use super::{
    ModelCallRecord, ModelFailure,
    embedding::{ResponseContract, invalid, normalize, validate_model},
};
use crate::network::NetworkClients;
use genai::ServiceTarget;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Serialize)]
pub(super) struct Request {
    pub model: String,
    pub input: Vec<String>,
    pub dimensions: usize,
    pub encoding_format: &'static str,
    #[serde(flatten)]
    pub parameters: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
struct Response {
    data: Vec<Embedding>,
    model: String,
}
#[derive(Deserialize)]
struct Embedding {
    embedding: Vec<f32>,
    index: usize,
}

pub(super) async fn execute<M>(
    network: &NetworkClients,
    target: ServiceTarget,
    request: Request,
    record: &mut ModelCallRecord<M>,
    contract: &ResponseContract,
) -> Result<Vec<Vec<f32>>, ModelFailure> {
    let body = super::embedding_http::exchange(
        network,
        target,
        "embeddings",
        &request,
        record,
        contract.max_bytes,
    )
    .await?;
    let response: Response = serde_json::from_value(body).map_err(|error| {
        invalid("embedding response does not match the vector contract")
            .with_source(Box::new(error))
    })?;
    validate_model(&response.model, &contract.space)?;
    if response.data.len() != contract.inputs {
        return Err(invalid(
            "embedding response must cover every input exactly once",
        ));
    }
    let mut ordered = vec![None; contract.inputs];
    for item in response.data {
        if item.index >= contract.inputs || ordered[item.index].is_some() {
            return Err(invalid(
                "embedding response contains a duplicate or out-of-range index",
            ));
        }
        ordered[item.index] = Some(normalize(item.embedding, &contract.space)?);
    }
    ordered
        .into_iter()
        .map(|vector| vector.ok_or_else(|| invalid("embedding response is missing an input index")))
        .collect()
}
