// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::*;
use nous_motivation::*;
use nous_runtime::{ContextResolver, DomainTemporalExtent};
use prost::Message;
use serde::Deserialize;
use std::collections::BTreeMap;
fn input(v: p::EvaluateConditionRequest) -> Result<EvaluateCondition> {
    Ok(EvaluateCondition {
        subject: SubjectId(id(&v.subject_id)?),
        operation_id: OperationId(id(&v.operation_id)?),
        condition_revision: from_ref(required(v.condition_revision, "condition revision")?)?,
        scope: super::motivation::scope(v.scope.unwrap_or_default())?,
        source_read: super::motivation::read_scope(v.source_read)?,
        evidence_refs: v
            .evidence_refs
            .into_iter()
            .map(from_ref)
            .collect::<Result<_>>()?,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    request_digest: String,
    leaves: Vec<SavedLeaf>,
    output: SemanticOutput,
    producer_bytes: Vec<u8>,
    evaluated_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Deserialize)]
struct SavedLeaf {
    key: String,
    materials: Vec<SavedMaterial>,
}
#[derive(Deserialize)]
struct SavedMaterial {
    key: String,
    reference: SavedRef,
}
#[derive(Deserialize)]
struct SavedRef {
    kind: String,
    value: String,
}
#[derive(Deserialize)]
struct SemanticOutput {
    results: Vec<SemanticResult>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticResult {
    key: String,
    truth: Truth,
    reason: String,
    evidence_keys: Vec<String>,
}
struct RecordedReader {
    official: OfficialCriterionReader,
    results: BTreeMap<String, CriterionRead>,
}
#[async_trait::async_trait]
impl CriterionReader for RecordedReader {
    async fn read_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
        c: &Criterion,
        interval: &DomainTemporalExtent,
        evidence: &[CognitiveRef],
    ) -> Result<CriterionRead> {
        if matches!(c, Criterion::Semantic { .. }) {
            let key = canonical_request_digest("motivation.semantic-leaf", subject, c)?;
            return self.results.get(&key).cloned().ok_or_else(|| {
                Error::FailedPrecondition(
                    "Paid semantic result does not cover the exact criterion".into(),
                )
            });
        }
        self.official
            .read_in(tx, subject, c, interval, evidence)
            .await
    }
}
#[tonic::async_trait]
impl k::kernel_motivation_service_server::KernelMotivationService for KernelService {
    async fn prepare(
        &self,
        r: Request<p::EvaluateConditionRequest>,
    ) -> std::result::Result<Response<k::ConditionEvaluationPlan>, Status> {
        rpc_reply(async {
            let input = input(r.into_inner())?;
            let plan = self
                .0
                .require_motivation()?
                .prepare_semantic(&input)
                .await?;
            let configuration = self.0.configuration.snapshot_for_subject(input.subject)?;
            let mut leaves = vec![];
            let mut used = 0;
            for leaf in plan.leaves {
                let refs = leaf
                    .references
                    .iter()
                    .cloned()
                    .map(|r| (r, false))
                    .collect::<Vec<_>>();
                let sources = self
                    .0
                    .context_sources(
                        input.subject,
                        &refs,
                        1024 * 1024,
                        self.0.cognition.now(input.subject),
                        &configuration,
                    )
                    .await?;
                let mut materials = vec![];
                for (index, source) in sources.into_iter().enumerate() {
                    let source = source?;
                    let text = source.text.ok_or_else(|| {
                        Error::Unavailable("Semantic criterion material text is unavailable".into())
                    })?;
                    used += text.len();
                    if used > 1024 * 1024 {
                        return Err(Error::Unavailable(
                            "Semantic criterion materials exceed bounded input".into(),
                        ));
                    }
                    materials.push(k::CriterionMaterial {
                        key: format!("source:{index}"),
                        reference: Some(to_ref(leaf.references[index].clone())),
                        source_occurrence: if let CognitiveRef::Occurrence(id) =
                            leaf.references[index]
                        {
                            Some(super::evidence::occurrence(
                                self.0.material.occurrence(input.subject, id).await?,
                            ))
                        } else {
                            None
                        },
                        text,
                    });
                }
                leaves.push(k::SemanticCriterionInput {
                    key: leaf.key,
                    question: leaf.question,
                    materials,
                });
            }
            Ok(k::ConditionEvaluationPlan {
                request_digest: plan.request_digest,
                leaves,
                evaluated: Some(super::motivation::stamp_proto(plan.evaluated)),
                dependencies: plan.dependencies.into_iter().map(to_ref).collect(),
            })
        })
        .await
    }
    async fn apply(
        &self,
        r: Request<k::ApplyConditionEvaluationRequest>,
    ) -> std::result::Result<Response<p::ConditionEvaluation>, Status> {
        rpc_reply(async {
            let v = r.into_inner();
            let request = input(required(v.request, "evaluation request")?)?;
            let identity = required(v.identity, "saved model identity")?;
            if identity.subject_id != request.subject.0.to_string()
                || identity.owner != "motivation_evaluation"
                || identity.operation_key != request.operation_id.0.to_string()
            {
                return Err(Error::Invalid(
                    "Semantic evaluation identity mismatch".into(),
                ));
            }
            let record = self
                .0
                .store
                .read_model_operation(
                    request.subject,
                    &identity.owner,
                    &identity.operation_key,
                    &identity.semantic_digest,
                )
                .await?
                .ok_or_else(|| Error::NotFound("Paid semantic result unavailable".into()))?;
            let proposal = record.proposal.ok_or_else(|| {
                Error::FailedPrecondition(
                    "Semantic evaluation requires its saved paid proposal".into(),
                )
            })?;
            if proposal.purged {
                return Err(Error::NotFound("Semantic source was purged".into()));
            }
            let saved: Saved = serde_json::from_value(proposal.payload)
                .map_err(|_| Error::Invalid("Invalid saved semantic result".into()))?;
            if saved.request_digest
                != canonical_request_digest("motivation.evaluate", request.subject, &request)?
                || saved.output.results.len() != saved.leaves.len()
            {
                return Err(Error::Conflict("Saved semantic inputs changed".into()));
            }
            let producer = from_producer(
                p::ProducerSignature::decode(saved.producer_bytes.as_slice())
                    .map_err(|_| Error::Invalid("Invalid canonical semantic producer".into()))?,
            )?;
            let producer = nous_persistence::AuthorityStore::canonical_producer(&producer)?;
            if producer.operation != CapabilityOperation::SubjectReflectionText {
                return Err(Error::Invalid(
                    "Semantic evaluation requires shared subject reflection provenance".into(),
                ));
            }
            let mut results = BTreeMap::new();
            for result in saved.output.results {
                let leaf = saved
                    .leaves
                    .iter()
                    .find(|l| l.key == result.key)
                    .ok_or_else(|| {
                        Error::Invalid("Semantic result references unselected criterion".into())
                    })?;
                if result.reason.trim().is_empty()
                    || result.truth != Truth::Unknown && result.evidence_keys.is_empty()
                {
                    return Err(Error::Invalid(
                        "Semantic judgment needs actual catalog evidence and reason".into(),
                    ));
                }
                let refs = result
                    .evidence_keys
                    .iter()
                    .map(|key| {
                        let item =
                            leaf.materials
                                .iter()
                                .find(|m| m.key == *key)
                                .ok_or_else(|| {
                                    Error::Invalid(
                                        "Semantic source key is outside actual materials".into(),
                                    )
                                })?;
                        parse_reference(&item.reference.kind, &item.reference.value)
                    })
                    .collect::<Result<Vec<_>>>()?;
                if results
                    .insert(
                        result.key,
                        CriterionRead {
                            evaluator: "semantic/v1".into(),
                            samples: vec![CriterionSample {
                                at: saved.evaluated_at,
                                truth: result.truth,
                                evidence: refs,
                            }],
                            complete: false,
                            method: format!("recorded_model: {}", result.reason),
                            producer: Some(producer.clone()),
                        },
                    )
                    .is_some()
                {
                    return Err(Error::Invalid("Duplicate semantic result".into()));
                }
            }
            let reader = RecordedReader {
                official: OfficialCriterionReader {
                    material: self.0.material.clone(),
                    social: self.0.social_cognition.clone(),
                },
                results,
            };
            Ok(super::motivation::evaluation_proto(
                self.0
                    .require_motivation()?
                    .evaluate(request, &reader)
                    .await?,
            ))
        })
        .await
    }
}
