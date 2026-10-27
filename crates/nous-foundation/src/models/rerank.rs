use super::{ModelCallRecord, ModelCapability, ModelFailure, ModelModality, ResolvedRoute};
use crate::network::NetworkClients;
use genai::ServiceTarget;
use nous_domain::{Error, ErrorCode, Result, UsageKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RerankScore {
    pub index: usize,
    pub relevance_score: f32,
}

#[derive(Clone, Serialize)]
pub(super) struct Request {
    model: String,
    query: String,
    documents: Vec<String>,
    top_n: usize,
    return_documents: bool,
    #[serde(flatten)]
    parameters: BTreeMap<String, Value>,
    #[serde(skip)]
    max_response_bytes: usize,
}

pub(super) fn validate(
    route: &ResolvedRoute,
    query: &str,
    documents: &[String],
    top_n: usize,
) -> Result<Request> {
    if !route.capabilities.contains(&ModelCapability::Rerank)
        || !route.input_modalities.contains(&ModelModality::Text)
    {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "route does not support text reranking",
        ));
    }
    if query.trim().is_empty()
        || documents.is_empty()
        || top_n == 0
        || top_n > documents.len()
        || documents
            .iter()
            .any(|text| text.trim().is_empty() || text.contains('\0'))
        || query.contains('\0')
    {
        return Err(Error::invalid(
            "rerank requires a query, nonempty documents and a valid top_n",
        ));
    }
    let bytes = documents
        .iter()
        .try_fold(query.len(), |sum, text| sum.checked_add(text.len()))
        .ok_or_else(|| Error::new(ErrorCode::ResourceExhausted, "rerank input length overflow"))?;
    if documents.len() > route.parameters.max_batch_size || bytes > route.parameters.max_input_bytes
    {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "rerank input exceeds the route document or byte limit",
        ));
    }
    Ok(Request {
        model: route.resource.model.clone(),
        query: query.into(),
        documents: documents.to_vec(),
        top_n,
        return_documents: false,
        parameters: route.parameters.native_parameters.clone(),
        max_response_bytes: route.parameters.max_response_bytes,
    })
}

pub(super) async fn execute<M>(
    network: &NetworkClients,
    target: ServiceTarget,
    request: Request,
    record: &mut ModelCallRecord<M>,
) -> std::result::Result<Vec<RerankScore>, ModelFailure> {
    let body = super::protocol_http::exchange(
        network,
        target,
        "rerank",
        &request,
        record,
        request.max_response_bytes,
        UsageKind::Rerank,
    )
    .await?;
    #[derive(Deserialize)]
    struct Response {
        results: Vec<WireScore>,
    }
    #[derive(Deserialize)]
    struct WireScore {
        index: usize,
        relevance_score: f32,
    }
    let response: Response = serde_json::from_value(body).map_err(|error| {
        ModelFailure::domain(Error::invalid(
            "rerank response requires indexed relevance scores",
        ))
        .with_source(Box::new(error))
    })?;
    if response.results.is_empty() || response.results.len() > request.top_n {
        return Err(ModelFailure::domain(Error::invalid(
            "rerank result count is outside requested top_n",
        )));
    }
    let mut indexes = BTreeSet::new();
    for score in &response.results {
        if score.index >= request.documents.len()
            || !indexes.insert(score.index)
            || !score.relevance_score.is_finite()
        {
            return Err(ModelFailure::domain(Error::invalid(
                "rerank scores require finite values and unique in-range indexes",
            )));
        }
    }
    let mut results = response
        .results
        .into_iter()
        .map(|score| RerankScore {
            index: score.index,
            relevance_score: score.relevance_score,
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .relevance_score
            .total_cmp(&left.relevance_score)
            .then(left.index.cmp(&right.index))
    });
    Ok(results)
}
