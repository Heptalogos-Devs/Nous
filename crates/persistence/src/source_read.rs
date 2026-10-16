//! Shared, parameterized SQL authorization over owner metadata in the caller's read view.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_core::*;
impl AuthorityStore {
    pub async fn source_read_metadata_closure_in(
        tx: &mut AuthorityTransaction<'_>,
        subject: SubjectId,
        sources: &[CognitiveRef],
    ) -> Result<Vec<(String, String, bool)>> {
        let (kinds, values): (Vec<_>, Vec<_>) = sources.iter().map(reference_parts).unzip();
        sqlx::query_as("WITH RECURSIVE walk(kind,value,path,cycle,metadata) AS (SELECT r.kind,r.value,ARRAY[r.kind||':'||r.value],false,false FROM unnest($2::text[],$3::text[]) r(kind,value) UNION ALL SELECT e.kind,e.value,w.path||(e.kind||':'||e.value),(e.kind||':'||e.value)=ANY(w.path),w.metadata OR e.metadata_edge FROM walk w CROSS JOIN LATERAL source_read_edges_with_mode($1,w.kind,w.value,w.metadata) e WHERE NOT w.cycle AND cardinality(w.path)<65) SELECT kind,value,cycle OR cardinality(path)>64 FROM walk LIMIT 1025").bind(subject.0).bind(kinds).bind(values).fetch_all(&mut **tx).await.map_err(database_error)
    }
    pub async fn source_read_allowed_in(
        tx: &mut AuthorityTransaction<'_>,
        view: &SourceReadView,
        reference: &CognitiveRef,
    ) -> Result<bool> {
        let (kind, value) = reference_parts(reference);
        sqlx::query_scalar("SELECT source_read_allowed($1,$2,$3,$4,$5,$6,$7)")
            .bind(view.subject.0)
            .bind(kind)
            .bind(value)
            .bind(&view.root_keys)
            .bind(&view.work_contexts)
            .bind(&view.origin_conversation_refs)
            .bind(serde_json::json!(&view.explicit_refs))
            .fetch_one(&mut **tx)
            .await
            .map_err(database_error)
    }
}

/// Owner-only static SQL columns; permissions precede ordering, ranking and body reads.
pub fn push_source_read_filter(
    sql: &mut sqlx::QueryBuilder<sqlx::Postgres>,
    view: Option<&SourceReadView>,
    kind_sql: &'static str,
    value_sql: &'static str,
) {
    if let Some(view) = view {
        sql.push(" AND source_read_allowed(")
            .push_bind(view.subject.0)
            .push(",")
            .push(kind_sql)
            .push(",")
            .push(value_sql)
            .push(",")
            .push_bind(view.root_keys.clone())
            .push(",")
            .push_bind(view.work_contexts.clone())
            .push(",")
            .push_bind(view.origin_conversation_refs.clone())
            .push(",")
            .push_bind(serde_json::json!(&view.explicit_refs))
            .push(")");
    }
}

impl AuthorityStore {
    /// Metadata SQL applies owner authorization before any refs leave PostgreSQL.
    /// This is a complete permission bitmap input, with no relevance/page truncation.
    pub async fn source_read_authorized_refs_in(
        tx: &mut AuthorityTransaction<'_>,
        view: &SourceReadView,
    ) -> Result<Vec<CognitiveRef>> {
        let rows:Vec<(String,String)>=sqlx::query_as("WITH refs(kind,value) AS (SELECT 'memory_revision',memory_revision_id::text FROM memory_revisions WHERE subject_id=$1 UNION ALL SELECT 'cognitive_schema_revision',r.schema_revision_id::text FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 UNION ALL SELECT 'episode_revision',episode_revision_id::text FROM episode_revisions WHERE subject_id=$1 UNION ALL SELECT 'journal_revision',journal_revision_id::text FROM journal_revisions WHERE subject_id=$1 UNION ALL SELECT o.family||'_revision',r.revision_id::text FROM self_revisions r JOIN self_objects o USING(object_id) WHERE r.subject_id=$1 UNION ALL SELECT o.family||'_revision',r.revision_id::text FROM social_revisions r JOIN social_objects o USING(object_id) WHERE r.subject_id=$1 UNION ALL SELECT 'association',association_evidence_id::text FROM association_evidence WHERE subject_id=$1 AND revoked_at IS NULL UNION ALL SELECT 'occurrence',occurrence_id::text FROM observation_occurrences WHERE subject_id=$1 UNION ALL SELECT 'artifact',artifact_id::text FROM artifacts WHERE subject_id=$1 UNION ALL SELECT 'source_region',source_region_id::text FROM source_regions WHERE subject_id=$1 UNION ALL SELECT 'derived_representation',derived_representation_id::text FROM derived_representations WHERE subject_id=$1 UNION ALL SELECT 'derived_region',derived_region_id::text FROM derived_regions WHERE subject_id=$1) SELECT kind,value FROM refs WHERE source_read_allowed($1,kind,value,$2,$3,$4,$5)").bind(view.subject.0).bind(&view.root_keys).bind(&view.work_contexts).bind(&view.origin_conversation_refs).bind(serde_json::json!(&view.explicit_refs)).fetch_all(&mut **tx).await.map_err(database_error)?;
        rows.into_iter()
            .map(|(kind, value)| parse_reference(&kind, &value))
            .collect()
    }
}
