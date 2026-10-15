// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
/// Mechanics for the Self owner's declared exact dependency edges. The caller
/// chooses the semantic source change; content/history are never rewritten.
impl AuthorityStore {
    pub async fn invalidate_self_dependents_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        kind: &str,
        values: &[String],
    ) -> Result<()> {
        sqlx::query("WITH RECURSIVE affected AS (SELECT o.object_id,o.current_revision_id,o.family FROM self_objects o JOIN self_dependencies d ON d.revision_id=o.current_revision_id WHERE o.subject_id=$1 AND d.ref_kind=$2 AND d.ref_value=ANY($3::text[]) UNION SELECT o.object_id,o.current_revision_id,o.family FROM self_objects o JOIN self_dependencies d ON d.revision_id=o.current_revision_id JOIN affected a ON d.ref_kind=a.family||'_revision' AND d.ref_value=a.current_revision_id::text WHERE o.subject_id=$1) UPDATE self_objects o SET integrity_state='revalidation_required',object_epoch=object_epoch+1 FROM affected a WHERE o.object_id=a.object_id AND o.integrity_state='valid'").bind(subject.0).bind(kind).bind(values).execute(&mut **tx).await.map_err(database_error)?;
        Ok(())
    }
}
