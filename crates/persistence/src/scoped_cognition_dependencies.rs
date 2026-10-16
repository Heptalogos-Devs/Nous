//! Shared mechanics for exact dependency edges declared by scoped cognition owners.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
impl AuthorityStore {
    pub async fn invalidate_scoped_cognition_dependents_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        kind: &str,
        values: &[String],
    ) -> Result<()> {
        sqlx::query(r#"
WITH RECURSIVE edges AS (
 SELECT 'self' owner,o.object_id,o.current_revision_id,o.family,d.ref_kind,d.ref_value FROM self_objects o JOIN self_dependencies d ON d.revision_id=o.current_revision_id WHERE o.subject_id=$1
 UNION ALL SELECT 'social',o.object_id,o.current_revision_id,o.family,d.ref_kind,d.ref_value FROM social_objects o JOIN social_dependencies d ON d.revision_id=o.current_revision_id WHERE o.subject_id=$1
), affected(owner,object_id,current_revision_id,family) AS (
 SELECT owner,object_id,current_revision_id,family FROM edges WHERE ref_kind=$2 AND ref_value=ANY($3::text[])
 UNION SELECT e.owner,e.object_id,e.current_revision_id,e.family FROM edges e JOIN affected a ON e.ref_kind=a.family||'_revision' AND e.ref_value=a.current_revision_id::text
), updated AS (
 UPDATE self_objects o SET integrity_state='revalidation_required',object_epoch=object_epoch+1 FROM affected a WHERE a.owner='self' AND o.object_id=a.object_id AND o.integrity_state='valid' RETURNING o.object_id
) UPDATE social_objects o SET integrity_state='revalidation_required',object_epoch=object_epoch+1 FROM affected a WHERE a.owner='social' AND o.object_id=a.object_id AND o.integrity_state='valid'
"#).bind(subject.0).bind(kind).bind(values).execute(&mut **tx).await.map_err(database_error)?;
        Ok(())
    }
    pub async fn scoped_cognition_dependency_references_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        kind: &str,
        values: &[String],
    ) -> Result<Vec<(String, String)>> {
        sqlx::query_as(r#"
WITH RECURSIVE edges AS (
 SELECT r.revision_id,o.family,d.ref_kind,d.ref_value FROM self_objects o JOIN self_revisions r USING(object_id) JOIN self_dependencies d USING(revision_id) WHERE o.subject_id=$1
 UNION ALL SELECT r.revision_id,o.family,d.ref_kind,d.ref_value FROM social_objects o JOIN social_revisions r USING(object_id) JOIN social_dependencies d USING(revision_id) WHERE o.subject_id=$1
), affected(revision_id,family) AS (
 SELECT revision_id,family FROM edges WHERE ref_kind=$2 AND ref_value=ANY($3::text[])
 UNION SELECT e.revision_id,e.family FROM edges e JOIN affected a ON e.ref_kind=a.family||'_revision' AND e.ref_value=a.revision_id::text
) SELECT family||'_revision',revision_id::text FROM affected ORDER BY family,revision_id
"#).bind(subject.0).bind(kind).bind(values).fetch_all(&mut **tx).await.map_err(database_error)
    }
}

impl AuthorityStore {
    /// Bounded reachability over owner-declared exact cognition edges. Used before
    /// attaching support to an existing revision, including mixed-owner cycles.
    pub async fn cognition_dependency_reaches_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        source: &nous_core::CognitiveRef,
        target: &nous_core::CognitiveRef,
    ) -> Result<bool> {
        let (source_kind, source_value) = nous_core::reference_parts(source);
        let (target_kind, target_value) = nous_core::reference_parts(target);
        let rows:Vec<(String,String)>=sqlx::query_as(r#"
WITH RECURSIVE edges(src_kind,src_value,dst_kind,dst_value) AS (
 SELECT o.family||'_revision',r.revision_id::text,d.ref_kind,d.ref_value FROM self_revisions r JOIN self_objects o USING(object_id) JOIN self_dependencies d USING(revision_id) WHERE o.subject_id=$1
 UNION ALL SELECT o.family||'_revision',r.revision_id::text,d.ref_kind,d.ref_value FROM social_revisions r JOIN social_objects o USING(object_id) JOIN social_dependencies d USING(revision_id) WHERE o.subject_id=$1
 UNION ALL SELECT 'memory_revision',r.memory_revision_id::text,d.target_ref_kind,d.target_ref FROM memory_revisions r JOIN memory_revision_dependencies d USING(memory_revision_id) WHERE r.subject_id=$1
 UNION ALL SELECT 'cognitive_schema_revision',r.schema_revision_id::text,d.basis_kind,d.basis_ref FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) JOIN cognitive_schema_evidence_links d USING(schema_revision_id) WHERE o.subject_id=$1
 UNION ALL SELECT 'episode_revision',r.episode_revision_id::text,d.basis_kind,d.basis_ref FROM episode_revisions r JOIN episode_revision_basis d USING(episode_revision_id) WHERE r.subject_id=$1
 UNION ALL SELECT 'journal_revision',r.journal_revision_id::text,d.basis#>>'{value,target_revision,kind}',d.basis#>>'{value,target_revision,id}' FROM journal_revisions r JOIN journal_objects o USING(journal_id) JOIN journal_point_basis d USING(journal_revision_id) WHERE o.subject_id=$1 AND d.basis->>'kind'='cognition_dependency'
), walk(kind,value) AS (
 SELECT $2::text,$3::text
 UNION SELECT e.dst_kind,e.dst_value FROM walk w JOIN edges e ON e.src_kind=w.kind AND e.src_value=w.value
) SELECT kind,value FROM walk LIMIT 1025
"#).bind(subject.0).bind(source_kind).bind(source_value).fetch_all(&mut **tx).await.map_err(database_error)?;
        if rows
            .iter()
            .any(|(k, v)| *k == target_kind && *v == target_value)
        {
            return Ok(true);
        }
        if rows.len() > 1024 {
            return Err(Error::Unavailable(
                "exact dependency closure exceeds 1024 source nodes".into(),
            ));
        }
        Ok(false)
    }
}
