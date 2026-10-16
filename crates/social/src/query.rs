// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_runtime::{BoundQuery, CognitiveContributor, LaneOutput, QueryPlan};
use std::collections::BTreeMap;
#[async_trait::async_trait]
impl CognitiveContributor for SocialService {
    fn owns(&self, reference: &CognitiveRef) -> bool {
        reference.domain() == CognitiveDomain::SocialCognition
    }
    async fn direct_lanes(
        &self,
        _bound: &BoundQuery,
        _plan: &QueryPlan,
    ) -> Result<Vec<LaneOutput>> {
        Ok(Vec::new())
    }
    async fn validate_and_materialize(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        bound: &BoundQuery,
    ) -> Result<(Vec<CognitiveHit>, BTreeMap<String, usize>)> {
        if subject != bound.source_query.subject {
            return Err(Error::Invalid("Social query Subject mismatch".into()));
        }
        let mut hits = Vec::new();
        let mut drops = BTreeMap::new();
        for reference in references {
            if !bound.social_authorized_refs.contains(reference) {
                *drops
                    .entry("social_source_read_scope_required".into())
                    .or_default() += 1;
                continue;
            }

            match self.query_record(subject, reference, bound).await {
                Ok(record) => {
                    let entities = record.content.entities();
                    let epistemic = EpistemicClass::Inferred;
                    let c = &bound.source_query.expression.constraints;
                    if !c.matches_common(QueryFacts {
                        authority: AuthorityClass::SubjectCognition,
                        entities: &entities,
                        source_classes: &[],
                        modality: Modality::Text,
                        cognitive_role: Some(record.content.family()),
                        formation_mode: None,
                        epistemic_class: Some(epistemic),
                    }) || c.recorded.is_some_and(|p| !p.contains(record.recorded_at))
                        || c.formed.is_some_and(|p| !p.contains(record.formed_at))
                        || c.valid
                            .is_some_and(|p| !p.matches_extent(record.content.valid_time()))
                        || c.observed.is_some()
                        || c.occurred.is_some()
                    {
                        *drops
                            .entry("social_constraint_ineligible".into())
                            .or_default() += 1;
                        continue;
                    }
                    let source = Self::record_context(&record, true);
                    hits.push(CognitiveHit {
                        authority_epoch: Some(record.object_epoch),
                        preference_refs: vec![record.object_reference()],
                        reference: reference.clone(),
                        revision: Some(record.reference()),
                        semantic_role: Some(record.content.family().into()),
                        cognitive_role: Some(record.content.family().into()),
                        formation_mode: None,
                        representation: source.text,
                        authority: AuthorityClass::SubjectCognition,
                        freshness: FreshnessDescriptor {
                            occurred: Vec::new(),
                            observed_at: None,
                            valid_time: record.content.valid_time().clone(),
                            formed_at: Some(record.formed_at),
                            recorded_at: Some(record.recorded_at),
                        },
                        entity_refs: entities,
                        evidence: source.evidence,
                        match_evidence: MatchEvidence {
                            families: Vec::new(),
                            base_rank_score: 0.0,
                            preference_score: 0.0,
                            rerank_score: None,
                            baseline_rank: 0,
                            final_rank: 0,
                            best_lane_rank: 0,
                            enabled_lane_count: 0,
                            final_score: 0.0,
                            variants: Vec::new(),
                            explanation: None,
                        },
                        materialization: Vec::new(),
                    });
                }
                Err(Error::NotFound(_) | Error::Unavailable(_)) => {
                    *drops.entry("social_unavailable".into()).or_default() += 1;
                }
                Err(e) => return Err(e),
            }
        }
        Ok((hits, drops))
    }
}
impl SocialService {
    async fn query_record(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        bound: &BoundQuery,
    ) -> Result<SocialRecord> {
        let record = if let Some(view) = &bound.historical_authority {
            let state = view
                .cognition_for(reference)
                .ok_or_else(|| Error::NotFound("Social is outside historical view".into()))?;
            let selected = if reference.is_cognition_revision() {
                reference
            } else {
                &state.head
            };
            if !state.revisions.contains(selected) {
                return Err(Error::NotFound(
                    "Social revision is outside historical selection".into(),
                ));
            }
            let mut record = self
                .get(subject, selected.uuid().expect("Social UUID"), true)
                .await?;
            if !record.is_current_available() {
                return Err(Error::Unavailable(
                    "Current Self lifecycle denies historical transmission".into(),
                ));
            }
            record.current_revision_id = state.head.uuid().expect("Social UUID");
            record.object_epoch = state.state["object_epoch"]
                .as_i64()
                .ok_or_else(|| Error::Infrastructure("Social historical epoch missing".into()))?;
            record.acceptance_state = state.state["acceptance_state"]
                .as_str()
                .unwrap_or("unavailable")
                .into();
            record.integrity_state = state.state["integrity_state"]
                .as_str()
                .unwrap_or("unavailable")
                .into();
            record.suppression_state = state.state["suppression_state"]
                .as_str()
                .unwrap_or("unavailable")
                .into();
            record.evidence.retain(|e| e.recorded_at <= view.as_of);
            record
        } else {
            self.get(
                subject,
                reference.uuid().expect("Social UUID"),
                reference.is_cognition_revision(),
            )
            .await?
        };
        let actual = if reference.is_cognition_revision() {
            record.reference()
        } else {
            record.object_reference()
        };
        if actual != *reference || !record.is_current_available() {
            return Err(Error::Unavailable(
                "Social exact identity/lifecycle is unavailable".into(),
            ));
        }
        if bound.historical_authority.is_none()
            && record.revision_id != record.current_revision_id
            && !bound.revision_policy.allows_historical(reference)
        {
            return Err(Error::Unavailable("Social revision is not current".into()));
        }
        // Query never truncates a semantic unit to fit a display budget.
        record.content.validate()?;
        Ok(record)
    }
    pub async fn query_descriptor(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<QueryDescriptor> {
        let record = self
            .get(
                subject,
                reference.uuid().expect("Social UUID"),
                reference.is_cognition_revision(),
            )
            .await?;
        if !record.is_current_available() {
            return Err(Error::Unavailable(
                "Social descriptor is unavailable".into(),
            ));
        }
        Ok(QueryDescriptor {
            reference: record.reference(),
            text: record.content.text().chars().take(2048).collect(),
        })
    }
}
