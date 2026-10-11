// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::{BTreeSet, VecDeque};

impl MemoryService {
    pub(crate) async fn invalidate_object_dependents_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        kind: &str,
        objects: &[Uuid],
        sequence: i64,
        reason: &str,
    ) -> Result<()> {
        let object_kind = match kind {
            "schema" => "cognitive_schema",
            "memory" | "cognitive_schema" | "episode" | "journal" => kind,
            _ => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InternalFailure,
                    "unknown source cognition kind",
                ));
            }
        };
        let revisions: Vec<Uuid> = sqlx::query_scalar(
            "SELECT revision_id FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind=$2 AND object_id=ANY($3::uuid[])",
        )
        .bind(subject.0)
        .bind(object_kind)
        .bind(objects)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        let revision_kind = format!("{object_kind}_revision");
        self.invalidate_cognition_dependents_in(
            tx,
            subject,
            &revision_kind,
            &revisions,
            sequence,
            reason,
        )
        .await
    }

    pub(crate) async fn invalidate_cognition_dependents_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        kind: &str,
        revisions: &[Uuid],
        sequence: i64,
        reason: &str,
    ) -> Result<()> {
        let mut pending: VecDeque<_> = revisions.iter().map(|id| (kind.to_owned(), *id)).collect();
        let roots: BTreeSet<_> = pending.iter().cloned().collect();
        let mut seen = BTreeSet::new();
        let now = self.cognition.now(subject);
        let mut affects_memory = false;
        let mut affects_journal = false;
        while let Some((kind, source)) = pending.pop_front() {
            if !seen.insert((kind.clone(), source)) {
                continue;
            }
            let edge_reason = if roots.contains(&(kind.clone(), source)) {
                reason
            } else {
                "upstream_invalidated"
            };
            let rows = sqlx::query(r"
SELECT 'memory_revision' AS kind,o.object_id AS object_id,o.current_revision_id AS revision
FROM cognition_objects o JOIN memory_revision_dependencies d ON d.memory_revision_id=o.current_revision_id
WHERE o.subject_id=$1 AND o.object_kind='memory' AND o.purge_state='normal' AND d.target_ref_kind=$2 AND d.target_ref=$3
UNION
SELECT 'cognitive_schema_revision',o.object_id,o.current_revision_id
FROM cognition_objects o JOIN cognitive_schema_evidence_links d ON d.schema_revision_id=o.current_revision_id
WHERE o.subject_id=$1 AND o.object_kind='cognitive_schema' AND o.purge_state='normal' AND d.basis_kind=$2 AND d.basis_ref=$3 AND d.revoked_at IS NULL
UNION
SELECT 'episode_revision',o.object_id,o.current_revision_id
FROM cognition_objects o JOIN episode_revision_basis d ON d.episode_revision_id=o.current_revision_id
WHERE o.subject_id=$1 AND o.object_kind='episode' AND o.purge_state='normal' AND d.basis_kind=$2 AND d.basis_ref=$3
UNION
SELECT 'episode_revision',o.object_id,o.current_revision_id
FROM cognition_objects o JOIN episode_revision_members d ON d.episode_revision_id=o.current_revision_id
WHERE o.subject_id=$1 AND o.object_kind='episode' AND o.purge_state='normal' AND d.ref_kind=$2 AND d.ref_value=$3
UNION
SELECT 'journal_revision',o.object_id,o.current_revision_id
FROM cognition_objects o JOIN journal_revision_sources d ON d.journal_revision_id=o.current_revision_id
WHERE o.subject_id=$1 AND o.object_kind='journal' AND o.purge_state='normal' AND d.ref_kind=$2 AND d.ref_value=$3
UNION
SELECT 'journal_revision',o.object_id,o.current_revision_id
FROM cognition_objects o JOIN journal_point_basis d ON d.journal_revision_id=o.current_revision_id
WHERE o.subject_id=$1 AND o.object_kind='journal' AND o.purge_state='normal' AND d.basis->>'kind'='cognition_dependency'
    AND d.basis#>>'{value,target_revision,kind}'=$2 AND d.basis#>>'{value,target_revision,id}'=$3
ORDER BY kind,object_id")
                .bind(subject.0).bind(&kind).bind(source.to_string()).fetch_all(&mut **tx).await.map_err(db)?;
            for row in rows {
                let dependent_kind: String = row.get("kind");
                let object: Uuid = row.get("object_id");
                let revision: Uuid = row.get("revision");
                sqlx::query("INSERT INTO cognition_dependency_invalidations(subject_id,dependent_kind,dependent_ref,invalidated_by_kind,invalidated_by_ref,reason,created_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(dependent_kind,dependent_ref,invalidated_by_kind,invalidated_by_ref) DO UPDATE SET reason=excluded.reason,created_at=excluded.created_at")
                    .bind(subject.0).bind(&dependent_kind).bind(revision.to_string()).bind(&kind).bind(source.to_string()).bind(edge_reason).bind(now)
                    .execute(&mut **tx).await.map_err(db)?;
                let dependent_object_kind = match dependent_kind.as_str() {
                    "memory_revision" => "memory",
                    "cognitive_schema_revision" => "cognitive_schema",
                    "episode_revision" => "episode",
                    "journal_revision" => "journal",
                    _ => {
                        return Err(Error::new(
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InternalFailure,
                            "unknown dependent cognition kind",
                        ));
                    }
                };
                let statement = "UPDATE cognition_objects SET integrity_state='revalidation_required',object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 AND integrity_state<>'revalidation_required'";
                let changed = sqlx::query(statement)
                    .bind(subject.0)
                    .bind(dependent_object_kind)
                    .bind(object)
                    .execute(&mut **tx)
                    .await
                    .map_err(db)?
                    .rows_affected()
                    > 0;
                affects_memory |= changed
                    && matches!(
                        dependent_kind.as_str(),
                        "memory_revision" | "cognitive_schema_revision"
                    );
                affects_journal |= changed
                    && matches!(
                        dependent_kind.as_str(),
                        "journal_revision" | "episode_revision"
                    );
                if dependent_kind == "journal_revision" {
                    self.cognition
                        .enqueue_maintenance_in(
                            tx,
                            &nous_runtime::MaintenanceRequest {
                                subject,
                                kind: "journal_revalidate".into(),
                                scope_kind: "journal".into(),
                                scope_ref: object.to_string(),
                                trigger_authority_seq: sequence,
                                due_at: now,
                                priority: 80,
                            },
                        )
                        .await?;
                }
                pending.push_back((dependent_kind, revision));
            }
        }
        if affects_memory || affects_journal {
            let families = if affects_memory {
                ProjectionInvalidation::all()
            } else {
                ProjectionInvalidation::text()
            };
            AuthorityStore::mark_projection_families_in(tx, subject, sequence, families).await?;
        }
        Ok(())
    }
}
