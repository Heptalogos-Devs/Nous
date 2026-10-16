//! Storage mechanics for the independent story-knowledge and real-recording cuts.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use sqlx::Row;
use std::collections::HashSet;

impl AuthorityStore {
    /// Apply the semantic cut to an owner-projected real-history snapshot. Late
    /// canonical virtual contacts remain visible when their story observation
    /// precedes the cut; the snapshot's real as_of still fences actual recording.
    pub async fn project_world_knowledge(
        &self,
        view: &mut HistoricalAuthoritySnapshot,
        cut: &DomainStamp,
    ) -> Result<()> {
        if cut.time_domain_ref == "time:real" || cut.time_domain_ref.trim().is_empty() {
            return Err(Error::Invalid(
                "knowledgeAtWorldTime requires a world domain".into(),
            ));
        }
        let mut tx = self.read_view().await?;
        let rows = sqlx::query(r#"
WITH revisions(kind,id,domain,formed) AS (
 SELECT 'memory_revision',memory_revision_id,formed_time_domain_ref,formed_at FROM memory_revisions WHERE subject_id=$1
 UNION ALL SELECT 'cognitive_schema_revision',r.schema_revision_id,r.formed_time_domain_ref,r.formed_at FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1
 UNION ALL SELECT 'episode_revision',episode_revision_id,formed_time_domain_ref,formed_at FROM episode_revisions WHERE subject_id=$1
 UNION ALL SELECT 'journal_revision',journal_revision_id,formed_time_domain_ref,formed_at FROM journal_revisions WHERE subject_id=$1
 UNION ALL SELECT o.family||'_revision',r.revision_id,r.formed_time_domain_ref,r.formed_at FROM self_revisions r JOIN self_objects o USING(object_id) WHERE r.subject_id=$1
 UNION ALL SELECT o.family||'_revision',r.revision_id,r.formed_time_domain_ref,r.formed_at FROM social_revisions r JOIN social_objects o USING(object_id) WHERE r.subject_id=$1
 UNION ALL SELECT o.family||'_revision',r.revision_id,r.formed_stamp->>'time_domain_ref',r.formed_at FROM motivation_revisions r JOIN motivation_objects o USING(object_id) WHERE r.subject_id=$1
) SELECT kind,id FROM revisions WHERE domain=$2 AND formed<=$3
"#).bind(view.subject.0).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut *tx).await.map_err(db)?;
        let known = rows
            .into_iter()
            .map(|row| {
                parse_reference(
                    &row.get::<String, _>("kind"),
                    &row.get::<uuid::Uuid, _>("id").to_string(),
                )
            })
            .collect::<Result<HashSet<_>>>()?;
        let mut cognition = Vec::new();
        for mut state in std::mem::take(&mut view.cognition) {
            state.recorded_revisions.retain(|r| known.contains(r));
            let Some(head) = state.recorded_revisions.first().cloned() else {
                continue;
            };
            let family = match state.object.domain() {
                CognitiveDomain::SelfCognition => "self",
                CognitiveDomain::SocialCognition => "social",
                CognitiveDomain::Motivation => "motivation",
                _ => state.object.kind(),
            };
            let historical:Option<serde_json::Value> = sqlx::query_scalar("SELECT state FROM authority_object_states WHERE subject_id=$1 AND object_kind=$2 AND object_ref=$3 AND recorded_at<=$4 AND semantic_stamp->>'time_domain_ref'=$5 AND (semantic_stamp->>'at')::timestamptz<=$6 ORDER BY recorded_at DESC,event_id DESC LIMIT 1")
                .bind(view.subject.0).bind(family).bind(reference_parts(&state.object).1).bind(view.as_of).bind(&cut.time_domain_ref).bind(cut.at).fetch_optional(&mut *tx).await.map_err(db)?;
            let Some(mut historical) = historical else {
                continue;
            };
            historical["current_revision_id"] = serde_json::json!(reference_parts(&head).1);
            state.state = historical;
            state.head = head.clone();
            state.revisions = if view.revision_view == RevisionView::Current {
                vec![head]
            } else {
                state.recorded_revisions.clone()
            };
            cognition.push(state);
        }
        view.cognition = cognition;
        let occurrences = sqlx::query("SELECT occurrence_id,artifact_id FROM observation_occurrences WHERE subject_id=$1 AND created_at<=$2 AND observed_stamp->>'time_domain_ref'=$3 AND (observed_stamp->>'at')::timestamptz<=$4")
            .bind(view.subject.0).bind(view.as_of).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut *tx).await.map_err(db)?;
        let mut material = HashSet::new();
        let mut artifacts = Vec::new();
        for row in occurrences {
            material.insert(CognitiveRef::Occurrence(OccurrenceId(
                row.get("occurrence_id"),
            )));
            if let Some(id) = row.get::<Option<uuid::Uuid>, _>("artifact_id") {
                artifacts.push(id);
                material.insert(CognitiveRef::Artifact(ArtifactId(id)));
            }
        }
        let regions:Vec<uuid::Uuid> = sqlx::query_scalar("SELECT source_region_id FROM source_regions WHERE subject_id=$1 AND artifact_id=ANY($2::uuid[]) AND created_at<=$3")
            .bind(view.subject.0).bind(&artifacts).bind(view.as_of).fetch_all(&mut *tx).await.map_err(db)?;
        for id in regions {
            material.insert(CognitiveRef::SourceRegion(SourceRegionId(id)));
        }
        let representations:Vec<uuid::Uuid> = sqlx::query_scalar("SELECT d.derived_representation_id FROM derived_representations d WHERE d.subject_id=$1 AND d.created_at<=$2 AND d.formed_stamp->>'time_domain_ref'=$4 AND (d.formed_stamp->>'at')::timestamptz<=$5 AND EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id)) AND NOT EXISTS(SELECT 1 FROM representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions s USING(source_region_id) WHERE NOT s.artifact_id=ANY($3::uuid[]))")
            .bind(view.subject.0).bind(view.as_of).bind(&artifacts).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut *tx).await.map_err(db)?;
        for id in &representations {
            material.insert(CognitiveRef::DerivedRepresentation(
                DerivedRepresentationId(*id),
            ));
        }
        let regions:Vec<uuid::Uuid> = sqlx::query_scalar("SELECT derived_region_id FROM derived_regions WHERE subject_id=$1 AND derived_representation_id=ANY($2::uuid[]) AND created_at<=$3 AND formed_stamp->>'time_domain_ref'=$4 AND (formed_stamp->>'at')::timestamptz<=$5")
            .bind(view.subject.0).bind(&representations).bind(view.as_of).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut *tx).await.map_err(db)?;
        for id in regions {
            material.insert(CognitiveRef::DerivedRegion(DerivedRegionId(id)));
        }
        view.material_visibility.retain(|r| material.contains(r));
        view.material_documents.retain(|r| material.contains(r));
        Self::project_knowledge_concepts_in(&mut tx, view, cut).await?;
        let visible: HashSet<_> =
            view.material_visibility
                .iter()
                .cloned()
                .chain(view.tags.iter().map(|tag| CognitiveRef::Tag(tag.tag)))
                .chain(view.cognition.iter().flat_map(|s| {
                    std::iter::once(s.object.clone()).chain(s.revisions.iter().cloned())
                }))
                .map(|r| reference_parts(&r))
                .collect();
        view.lexical_visibility
            .retain(|r| visible.contains(&(r.object_kind.clone(), r.canonical_ref.clone())));
        view.knowledge_at_world_time = Some(cut.clone());
        view.refresh_digest()?;
        Ok(())
    }
    async fn project_knowledge_concepts_in(
        tx: &mut crate::AuthorityTransaction<'_>,
        view: &mut HistoricalAuthoritySnapshot,
        cut: &DomainStamp,
    ) -> Result<()> {
        let tags = sqlx::query("SELECT t.tag_id,h.state,r.tag_revision_id,r.label,r.description,r.kind_hint FROM tags t JOIN LATERAL (SELECT state FROM authority_object_states WHERE subject_id=$1 AND object_kind='tag' AND object_ref=t.tag_id::text AND recorded_at<=$2 AND semantic_stamp->>'time_domain_ref'=$3 AND (semantic_stamp->>'at')::timestamptz<=$4 ORDER BY recorded_at DESC,event_id DESC LIMIT 1) h ON true JOIN tag_revisions r ON r.tag_revision_id=(h.state->>'current_revision_id')::uuid WHERE t.subject_id=$1")
            .bind(view.subject.0).bind(view.as_of).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut **tx).await.map_err(db)?;
        view.tags = tags
            .into_iter()
            .map(|row| {
                let state: serde_json::Value = row.get("state");
                Ok(HistoricalTagState {
                    tag: TagId(row.get("tag_id")),
                    revision_id: row.get("tag_revision_id"),
                    canonical_tag: state["canonical_tag_id"]
                        .as_str()
                        .map(str::parse::<uuid::Uuid>)
                        .transpose()
                        .map_err(|error| Error::Infrastructure(error.to_string()))?
                        .map(TagId),
                    status: state["status"]
                        .as_str()
                        .ok_or_else(|| Error::Infrastructure("Tag state missing status".into()))?
                        .into(),
                    semantic: tag_semantic_representation(
                        &row.get::<String, _>("label"),
                        row.get::<Option<String>, _>("description").as_deref(),
                        row.get::<Option<String>, _>("kind_hint").as_deref(),
                    )?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let associations:Vec<uuid::Uuid> = sqlx::query_scalar("SELECT association_evidence_id FROM association_evidence WHERE subject_id=$1 AND formed_stamp->>'time_domain_ref'=$2 AND (formed_stamp->>'at')::timestamptz<=$3")
            .bind(view.subject.0).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut **tx).await.map_err(db)?;
        view.associations.retain(|id| associations.contains(&id.0));
        let links:Vec<uuid::Uuid> = sqlx::query_scalar("SELECT link_id FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND formed_stamp->>'time_domain_ref'=$2 AND (formed_stamp->>'at')::timestamptz<=$3")
            .bind(view.subject.0).bind(&cut.time_domain_ref).bind(cut.at).fetch_all(&mut **tx).await.map_err(db)?;
        view.schema_evidence_links.retain(|id| links.contains(id));
        Ok(())
    }
}
