//! Reverse evidence closure supplies lifecycle owners with exact affected identities.
use crate::{
    WriteTransaction,
    codec::{enum_name, parse_enum},
    storage_error,
};
use nous_domain::*;
use sqlx::{QueryBuilder, Row, Sqlite, sqlite::SqliteRow};
const REVERSE_LINKS: &str = "UNION SELECT r.kind,l.object_id,l.revision FROM structured_links l JOIN structured_revisions r ON r.object_id=l.object_id AND r.revision=l.revision JOIN affected a ON a.kind=l.source_kind AND a.id=l.source_id AND a.revision IS l.source_revision";
pub(crate) fn memory_dependents_cte(id: &MemoryId) -> QueryBuilder<Sqlite> {
    let mut query = QueryBuilder::new(
        "WITH RECURSIVE affected(kind,id,revision) AS(SELECT 'memory',memory_id,revision FROM memory_revisions WHERE memory_id=",
    );
    query
        .push_bind(id.as_str())
        .push(" ")
        .push(REVERSE_LINKS)
        .push(")");
    query
}
impl WriteTransaction {
    pub async fn media_source_dependents(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &media::MediaReference,
    ) -> Result<Vec<RetrievalTarget>> {
        super::references::media_source(&mut self.tx, namespace, subject, reference).await?;
        let ids:Vec<String>=sqlx::query_scalar("SELECT DISTINCT source_id FROM structured_links WHERE namespace_id=? AND subject_id=? AND source_kind='media'")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut references = std::collections::BTreeSet::from([reference.clone()]);
        for id in ids {
            let EvidenceSource::Media {
                reference: dependent,
            } = super::references::source_from_parts("media", id, None)?
            else {
                unreachable!()
            };
            match crate::media::lineage_in(&mut self.tx, namespace, subject, &dependent, 0).await {
                Ok(lineage) if lineage.contains(reference) => {
                    references.insert(dependent);
                }
                Ok(_)
                | Err(Error {
                    code: ErrorCode::NotFound,
                    ..
                }) => {}
                Err(error) => return Err(error),
            }
        }
        let roots=references.into_iter().map(|reference|Ok(serde_json::json!({"kind":"media","id":RetrievalTarget::Media(reference).key()?,"revision":null}))).collect::<Result<Vec<_>>>()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH RECURSIVE affected(kind,id,revision) AS(SELECT json_extract(value,'$.kind'),json_extract(value,'$.id'),json_extract(value,'$.revision') FROM json_each(",
        );
        query
            .push_bind(serde_json::to_string(&roots)?)
            .push(") ")
            .push(REVERSE_LINKS)
            .push(") SELECT kind,id,revision FROM affected ORDER BY kind,id,revision");
        query
            .build()
            .fetch_all(&mut *self.tx)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(target_row)
            .collect()
    }
    pub async fn memory_source_dependents(
        &mut self,
        namespace: &NamespaceId,
        id: &MemoryId,
    ) -> Result<Vec<RetrievalTarget>> {
        self.memory(namespace, id).await?;
        let mut query = memory_dependents_cte(id);
        query
            .push(" SELECT kind,id,revision FROM affected ORDER BY kind,id,revision")
            .build()
            .fetch_all(&mut *self.tx)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(target_row)
            .collect()
    }
    pub async fn cognition_source_dependents(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &CognitiveRef,
    ) -> Result<Vec<RetrievalTarget>> {
        self.validate_reference(namespace, subject, reference)
            .await?;
        let roots=if reference.kind==CognitiveKind::Memory {
            sqlx::query("SELECT 'memory' AS kind,memory_id AS id,revision FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=?")
                .bind(namespace.as_str()).bind(subject.as_str()).bind(&reference.id).fetch_all(&mut *self.tx).await.map_err(storage_error)?
        }else{
            sqlx::query("SELECT kind,object_id AS id,revision FROM structured_revisions WHERE namespace_id=? AND subject_id=? AND object_id=? AND kind=?")
                .bind(namespace.as_str()).bind(subject.as_str()).bind(&reference.id).bind(enum_name(&reference.kind)?).fetch_all(&mut *self.tx).await.map_err(storage_error)?
        }.into_iter().map(|row|match target_row(row)?{RetrievalTarget::Cognition(reference)=>Ok(reference),_=>unreachable!()}).collect::<Result<Vec<_>>>()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH RECURSIVE affected(kind,id,revision) AS(SELECT json_extract(value,'$.kind'),json_extract(value,'$.id'),json_extract(value,'$.revision') FROM json_each(",
        );
        query
            .push_bind(serde_json::to_string(&roots)?)
            .push(") ")
            .push(REVERSE_LINKS)
            .push(") SELECT kind,id,revision FROM affected ORDER BY kind,id,revision");
        query
            .build()
            .fetch_all(&mut *self.tx)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(target_row)
            .collect()
    }
}
fn target_row(row: SqliteRow) -> Result<RetrievalTarget> {
    if row.get::<&str, _>("kind") == "media" {
        let EvidenceSource::Media { reference } =
            super::references::source_from_parts("media", row.get("id"), None)?
        else {
            unreachable!()
        };
        return Ok(RetrievalTarget::Media(reference));
    }
    Ok(RetrievalTarget::Cognition(CognitiveRef {
        kind: parse_enum(row.get("kind"))?,
        id: row.get("id"),
        revision: row.get("revision"),
    }))
}
