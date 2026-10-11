// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;
use sqlx::Row;
use std::collections::BTreeMap;

const MAINTENANCE_TEXT_SAFETY_BYTES: usize = 65536;

impl KernelService {
    pub(super) async fn plan_maintenance(
        &self,
        input: k::PlanMaintenanceRequest,
    ) -> Result<k::MaintenancePlan> {
        let claimed = longitudinal::need(required(input.claimed, "claimed")?)?;
        self.require_plan_lease(&claimed).await?;
        let subject = claimed.subject_id;
        let member_bytes = self
            .0
            .configuration
            .snapshot_for_subject(subject)?
            .get(nous_runtime::MEMBER_TEXT_MAX_BYTES)?;
        let sequence = self.0.store.authority_seq(subject).await?;
        let memory = self.require_memory()?;
        if claimed.kind == "concept_maintenance" {
            return self.concept_maintenance_plan(&claimed).await;
        }
        let scope = match claimed.kind.as_str() {
            "episode_resegment" => {
                memory
                    .plan_episode_review(
                        subject,
                        &claimed.scope_kind,
                        &claimed.scope_ref,
                        claimed.trigger_authority_seq,
                    )
                    .await?
            }
            "journal_review" | "journal_revalidate" => {
                memory
                    .plan_journal_review(subject, &claimed.kind, &claimed.scope_ref)
                    .await?
            }
            "memory_consolidate" => {
                memory
                    .plan_consolidation_scope(subject, &claimed.scope_kind, &claimed.scope_ref)
                    .await?
            }
            _ => return Err(Error::Invalid("maintenance kind has no model plan".into())),
        };
        let source = if claimed.kind == "memory_consolidate" {
            consolidation_source(&scope)
        } else {
            None
        };
        let mut plan = k::MaintenancePlan {
            subject_id: subject.0.to_string(),
            authority_seq: sequence,
            cognitive_now: Some(timestamp(self.0.cognition.now(subject))),
            status: scope.status,
            next_due: scope.next_due.map(timestamp),
            problem_code: scope.problem,
            ..Default::default()
        };
        plan.consolidation_source = source;
        if let Some(journal) = scope.journal {
            plan.target = Some(k::JournalTarget {
                journal_id: journal.object.journal_id.0.to_string(),
                expected_revision_id: journal.revision.journal_revision_id.0.to_string(),
                expected_epoch: journal.object.object_epoch,
                intent: "revalidate".into(),
            });
            plan.journal = Some(journal::journal_revision_proto(journal));
        }
        let mut catalog = BTreeMap::new();
        for episode in &scope.episodes {
            plan.sources.push(k::EpisodePartitionSource {
                revision_id: episode.revision.episode_revision_id.0.to_string(),
                expected_epoch: episode.object.object_epoch,
            });
            let basis = RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: CognitiveRef::EpisodeRevision(
                    episode.revision.episode_revision_id,
                ),
                basis_role: BasisRole::Direct,
            });
            catalog.insert(basis.canonical_key(), basis);
            for basis in &episode.basis {
                catalog.insert(basis.canonical_key(), basis.clone());
            }
            for member in &episode.members {
                if let CognitiveRef::Occurrence(id) = member.reference {
                    let basis = RevisionBasis::Evidence(EvidenceRef {
                        epistemic_relation: None,
                        occurrence_id: id,
                        locator: EvidenceLocator::WholeOccurrence,
                        basis_role: BasisRole::Direct,
                    });
                    catalog.insert(basis.canonical_key(), basis);
                }
            }
        }
        plan.episodes = scope.episodes.into_iter().map(episode::view).collect();
        let (members, scope_exceeded) = self
            .maintenance_member_catalog(subject, &scope.occurrences, member_bytes as usize)
            .await?;
        plan.members = members;
        if claimed.kind == "episode_resegment" && scope_exceeded {
            plan.status = "blocked".into();
            plan.problem_code = Some("historical_repair_scope_exceeded".into());
            plan.next_due = None;
        }
        plan.basis = catalog
            .into_iter()
            .map(|(key, basis)| k::BasisCatalogEntry {
                key,
                basis: Some(basis_proto(basis)),
            })
            .collect();
        if claimed.kind == "memory_consolidate" && plan.status == "ready" {
            self.consolidation_context(&mut plan).await?;
        }
        if self.0.store.authority_seq(subject).await? != sequence {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "maintenance source snapshot changed during planning",
            )
            .into());
        }
        Ok(plan)
    }

    async fn require_plan_lease(&self, claimed: &nous_runtime::MaintenanceNeed) -> Result<()> {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND state='leased' AND lease_token=$3 AND lease_until>clock_timestamp() AND kind=$4 AND scope_kind=$5 AND scope_ref=$6)")
            .bind(claimed.subject_id.0).bind(claimed.need_id).bind(claimed.lease_token).bind(&claimed.kind).bind(&claimed.scope_kind).bind(&claimed.scope_ref).fetch_one(self.0.store.pool()).await.map_err(nous_persistence::database_error)?;
        if !valid {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::LeaseLost,
                "maintenance planning lease expired or replaced",
            )
            .into());
        }
        Ok(())
    }
    async fn concept_maintenance_plan(
        &self,
        claimed: &nous_runtime::MaintenanceNeed,
    ) -> Result<k::MaintenancePlan> {
        let subject = claimed.subject_id;
        let sequence = self.0.store.authority_seq(subject).await?;
        let memory = self.require_memory()?;
        {
            let focus = parse_reference(&claimed.scope_kind, &claimed.scope_ref)?;
            let concepts = match memory.plan_concepts(subject, focus).await {
                Ok(plan) => plan,
                Err(Error::NotFound(_)) => {
                    return Ok(k::MaintenancePlan {
                        subject_id: subject.0.to_string(),
                        authority_seq: sequence,
                        status: "obsolete".into(),
                        ..Default::default()
                    });
                }
                Err(error) => return Err(error),
            };
            Ok(k::MaintenancePlan {
                subject_id: subject.0.to_string(),
                authority_seq: concepts.authority_seq,
                cognitive_now: Some(timestamp(self.0.cognition.now(subject))),
                status: "ready".into(),
                concept_model_input_json: Some(concepts.model_input.to_string()),
                concept_catalog: Some(k::ConceptMaintenanceCatalog {
                    max_suggestions: concepts.policy.max_suggestions as u32,
                    config_digest: concepts.config_digest,
                    references: concepts
                        .references
                        .into_iter()
                        .map(|(key, reference)| k::ConceptReference {
                            key,
                            reference: Some(to_ref(reference)),
                        })
                        .collect(),
                    tags: concepts
                        .tags
                        .into_iter()
                        .map(|tag| k::ConceptTag {
                            key: tag.key,
                            target: Some(p::TagRevisionTarget {
                                tag_id: tag.target.tag_id.0.to_string(),
                                expected_revision_id: tag.target.expected_revision_id.to_string(),
                            }),
                        })
                        .collect(),
                    associations: concepts
                        .associations
                        .into_iter()
                        .map(|a| k::ConceptAssociation {
                            key: a.key,
                            association_id: a.id.0.to_string(),
                            from_key: a.from,
                            to_key: a.to,
                            relation: a.relation,
                        })
                        .collect(),
                    basis: concepts
                        .basis
                        .into_iter()
                        .map(|(key, basis)| k::ConceptBasis {
                            key,
                            basis: Some(super::concepts::association_basis_proto(basis)),
                        })
                        .collect(),
                }),
                ..Default::default()
            })
        }
    }
    async fn maintenance_member_catalog(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
        member_bytes: usize,
    ) -> Result<(Vec<k::ExperienceMember>, bool)> {
        let ids: Vec<uuid::Uuid> = occurrences.iter().map(|id| id.0).collect();
        let rows=sqlx::query("SELECT o.*,e.recorded_seq,e.session_id,e.active_work_context_id,e.active_work_context_revision,d.derived_representation_id FROM observation_occurrences o LEFT JOIN experience_items e ON e.occurrence_id=o.occurrence_id AND e.subject_id=o.subject_id LEFT JOIN LATERAL (SELECT r.derived_representation_id FROM coverage_needs c JOIN source_regions s USING(source_region_id) JOIN derived_representations r ON r.derived_representation_id=c.current_representation_id WHERE c.subject_id=$1 AND s.artifact_id=o.artifact_id AND c.state='ready' AND r.payload_text IS NOT NULL ORDER BY r.created_at DESC,r.derived_representation_id LIMIT 1) d ON true WHERE o.subject_id=$1 AND o.occurrence_id=ANY($2::uuid[]) ORDER BY o.observed_at,e.recorded_seq NULLS LAST,o.occurrence_id")
            .bind(subject.0).bind(ids).fetch_all(self.0.store.pool()).await.map_err(nous_persistence::database_error)?;
        let mut members = Vec::with_capacity(rows.len());
        let mut remaining = MAINTENANCE_TEXT_SAFETY_BYTES;
        let mut scope_exceeded = false;
        for row in rows {
            let occurrence: uuid::Uuid = row.get("occurrence_id");
            let derived: Option<uuid::Uuid> = row.get("derived_representation_id");
            let reference = derived
                .map(|id| CognitiveRef::DerivedRepresentation(DerivedRepresentationId(id)))
                .unwrap_or(CognitiveRef::Occurrence(OccurrenceId(occurrence)));
            let (text, partial) = if remaining > 0 {
                let source = self
                    .0
                    .materialize(
                        subject,
                        nous_material::MaterializeRequest {
                            reference,
                            byte_range: None,
                            max_bytes: remaining.min(member_bytes) as u64,
                            resource_handle: None,
                            resource: None,
                        },
                    )
                    .await?;
                if source.media_type.starts_with("text/") || source.media_type == "application/json"
                {
                    remaining = remaining.saturating_sub(source.bytes.len());
                    let text = experience_text_prefix(&source.bytes, source.partial)?;
                    (text.to_owned(), source.partial)
                } else {
                    (String::new(), true)
                }
            } else {
                scope_exceeded = true;
                (String::new(), true)
            };
            members.push(k::ExperienceMember {
                key: format!("occurrence:{occurrence}"),
                occurrence_id: occurrence.to_string(),
                recorded_seq: row.get("recorded_seq"),
                observed_at: Some(timestamp(row.get("observed_at"))),
                session_id: row
                    .get::<Option<uuid::Uuid>, _>("session_id")
                    .map(|id| id.to_string()),
                work_context_id: row
                    .get::<Option<uuid::Uuid>, _>("active_work_context_id")
                    .map(|id| id.to_string()),
                work_context_revision: row.get("active_work_context_revision"),
                conversation_ref: row.get("conversation_ref"),
                actor_entity_ref: row.get("actor_entity_ref"),
                source_class: row.get("source_class"),
                text,
                text_partial: partial,
                occurred_time: Some(occurrence_time(&row)?),
            });
        }
        Ok((members, scope_exceeded))
    }
}

fn consolidation_source(scope: &nous_memory::MaintenanceScope) -> Option<k::ExpectedCognition> {
    if let Some(journal) = &scope.journal {
        return Some(k::ExpectedCognition {
            reference: Some(to_ref(CognitiveRef::JournalRevision(
                journal.revision.journal_revision_id,
            ))),
            expected_epoch: journal.object.object_epoch,
            object_id: journal.object.journal_id.0.to_string(),
        });
    }
    scope.episodes.first().map(|episode| k::ExpectedCognition {
        reference: Some(to_ref(CognitiveRef::EpisodeRevision(
            episode.revision.episode_revision_id,
        ))),
        expected_epoch: episode.object.object_epoch,
        object_id: episode.object.episode_id.0.to_string(),
    })
}

fn occurrence_time(row: &sqlx::postgres::PgRow) -> Result<p::TemporalExtent> {
    let kind: String = row.get("occurred_time_kind");
    let start: Option<chrono::DateTime<chrono::Utc>> = row.get("occurred_time_start");
    let end: Option<chrono::DateTime<chrono::Utc>> = row.get("occurred_time_end");
    let time = match kind.as_str() {
        "instant" => TemporalExtent::Instant {
            at: required(start, "occurred instant")?,
        },
        "interval" => TemporalExtent::Interval { start, end },
        "unknown" => TemporalExtent::Unknown,
        _ => {
            return Err(Error::Infrastructure(
                "invalid occurrence temporal kind".into(),
            ));
        }
    };
    Ok(temporal_proto(&time))
}

fn experience_text_prefix(bytes: &[u8], partial: bool) -> Result<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if partial && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .map_err(|error| Error::Invalid(error.to_string()))
        }
        Err(error) => Err(Error::Invalid(format!("experience text encoding: {error}"))),
    }
}
