// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::Result;
use nous_retrieval::{QueryEmbedding, TextEmbeddingOutput};

impl KernelService {
    pub(super) fn embedding_config(&self) -> Result<k::EmbeddingConfig> {
        let provider = self
            .0
            .serving
            .embedding()
            .ok_or_else(|| Error::Unavailable("embedding space not configured".into()))?;
        let space = provider.space();
        Ok(k::EmbeddingConfig {
            space_hash: space.space_hash,
            producer_hash: provider.producer().signature_hash,
            model: space.model_identity,
            dimension: space.dimension,
            producer_hashes: provider
                .producers()
                .iter()
                .map(|p| p.signature_hash.clone())
                .collect(),
        })
    }
    fn query_materials(
        &self,
        bound: &nous_runtime::BoundQuery,
        inputs: Vec<k::QueryEmbedding>,
    ) -> Result<Vec<QueryEmbedding>> {
        if inputs.len() > 1 {
            return Err(Error::Invalid(
                "one prepared query accepts at most one embedding".into(),
            ));
        }
        let mut materials = vec![];
        for material in inputs {
            let provider = self
                .0
                .serving
                .embedding()
                .ok_or_else(|| Error::Unavailable("embedding space not configured".into()))?;
            let space = provider.space();
            let producer = provider
                .producers()
                .into_iter()
                .find(|p| p.signature_hash == material.producer_hash)
                .ok_or_else(|| {
                    Error::Invalid("query embedding producer is not authorized".into())
                })?;
            if material.text != bound.representation.text
                || material.text.len() > 131072
                || material.space_hash != space.space_hash
                || material.producer_hash != producer.signature_hash
                || material.vector.len() != space.dimension as usize
                || material.vector.iter().any(|v| !v.is_finite())
            {
                return Err(Error::Invalid(
                    "query embedding is incompatible with selected space/producer".into(),
                ));
            }
            materials.push(QueryEmbedding {
                text: material.text,
                output: TextEmbeddingOutput {
                    vector: material.vector,
                    space,
                    producer,
                },
            });
        }
        Ok(materials)
    }
    pub(super) async fn activate_query_with_material(
        &self,
        input: k::KernelQueryRequest,
    ) -> Result<k::QueryActivationResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let mut reservation = self
            .0
            .cognition
            .take_prepared_query(subject, id(&input.preparation_token)?)?;
        let bound = &mut reservation.bound;
        if bound.concept_enrichment != nous_runtime::ConceptEnrichment::Model
            || bound.source_query.capabilities.query_concept_enrichment
                == nous_core::RequirementStrength::Forbidden
            || bound.activation.frozen
            || input.concept_model_calls != 0
            || input.concept_output.is_some()
            || input.concept_failure.is_some()
        {
            return Err(Error::Invalid(
                "query concept model activity is not permitted".into(),
            ));
        }
        let materials = self.query_materials(bound, input.embeddings)?;
        let plan = nous_runtime::QueryPlan::for_bound_query(bound);
        use nous_runtime::SharedLaneProvider;
        let (projection, view, activation) =
            nous_retrieval::with_query_material(materials, async {
                let (projection, view) = self.0.serving.prepare_query(bound, &plan).await?;
                let activation = view.activate(bound).await?;
                Ok::<_, Error>((projection, view, activation))
            })
            .await?;
        bound.activation = activation;
        bound.activation.degradation.extend(projection.degradation);
        bound.activation.frozen = true;
        bound.activation_view = Some(view);
        let catalog = bound.activation.concept_catalog.iter().map(|candidate| serde_json::json!({"key":candidate.key,"semantic_text":candidate.semantic_text,"strength":candidate.strength})).collect::<Vec<_>>();
        let model_input = serde_json::json!({"query_representation":bound.representation,"existing_tags":catalog,"temporal_frame":bound.source_query.temporal_frame}).to_string();
        let token = self.0.cognition.retain_prepared_query(reservation)?;
        Ok(k::QueryActivationResponse {
            preparation_token: token.to_string(),
            model_input,
        })
    }
    fn apply_query_concept_output(
        &self,
        bound: &mut nous_runtime::BoundQuery,
        output: Option<String>,
        failure: Option<String>,
        calls: u32,
    ) -> Result<()> {
        if calls > 4
            || (output.is_some() && calls == 0)
            || (output.is_none() && failure.is_none() && calls != 0)
        {
            return Err(Error::Invalid(
                "invalid query concept model call accounting".into(),
            ));
        }
        if output.is_none() && failure.is_none() {
            return Ok(());
        }
        if bound.concept_enrichment != nous_runtime::ConceptEnrichment::Model
            || !bound.activation.frozen
            || bound.source_query.capabilities.query_concept_enrichment
                == nous_core::RequirementStrength::Forbidden
            || (output.is_some() && failure.is_some())
        {
            return Err(Error::Invalid(
                "unexpected query concept model result".into(),
            ));
        }
        if let Some(output) = output {
            if output.len() > 16384 {
                return Err(Error::Invalid("query concept output exceeds bounds".into()));
            }
            let output = serde_json::from_str(&output)
                .map_err(|_| Error::Invalid("invalid query concept output".into()))?;
            let policy =
                nous_runtime::ConceptActivationPolicy::from_snapshot(&bound.config_snapshot)?;
            bound.activation.apply_concept_model(output, &policy)?;
            bound.activation.model_calls = calls as usize;
        } else {
            bound.activation.model_calls = calls as usize;
            bound.activation.degradation.push(nous_core::Degradation {
                code: "query_concept_model_unavailable".into(),
                detail: Some(failure.unwrap_or_default().chars().take(1024).collect()),
            });
        }
        Ok(())
    }
    pub(super) async fn query_with_material(
        &self,
        input: k::KernelQueryRequest,
    ) -> Result<k::KernelQueryResponse> {
        let reservation = self.0.cognition.take_prepared_query(
            SubjectId(id(&input.subject_id)?),
            id(&input.preparation_token)?,
        )?;
        let (mut bound, lease) = reservation.into_parts();
        let materials = self.query_materials(&bound, input.embeddings)?;
        self.apply_query_concept_output(
            &mut bound,
            input.concept_output,
            input.concept_failure,
            input.concept_model_calls,
        )?;
        nous_retrieval::with_query_material(
            materials,
            self.query(
                bound,
                lease,
                input.validated_candidate_limit.map(|limit| limit as usize),
            ),
        )
        .await
    }
}

pub(super) fn workflow_json(value: &str) -> Result<serde_json::Value> {
    if value.len() > nous_persistence::WORKFLOW_VALUE_MAX_BYTES {
        return Err(Error::Invalid("workflow value exceeds bound".into()));
    }
    serde_json::from_str(value).map_err(|_| Error::Invalid("workflow JSON is invalid".into()))
}

pub(super) fn private_workflow_owner(owner: &str) -> Result<()> {
    nous_persistence::WorkflowOwner::new(owner)?;
    if ["memory", "material"].contains(&owner) {
        Ok(())
    } else {
        Err(Error::Invalid(
            "workflow owner has no private execution route".into(),
        ))
    }
}
