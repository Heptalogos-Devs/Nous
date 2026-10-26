use crate::{WriteTransaction, storage_error, write_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection};

pub(crate) async fn read(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    target: &RetrievalTarget,
) -> Result<Option<SourceTombstone>> {
    let body: Option<String> = sqlx::query_scalar(
        "SELECT body FROM source_tombstones WHERE namespace_id=? AND subject_id=? AND target_key=?",
    )
    .bind(ns.as_str())
    .bind(subject.as_str())
    .bind(target.key()?)
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?;
    body.map(|value| serde_json::from_str(&value).map_err(Error::from))
        .transpose()
}
pub(crate) async fn purged_sequence(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    reference: &CognitiveRef,
) -> Result<Option<i64>> {
    if reference.kind != CognitiveKind::Memory {
        return Ok(None);
    }
    sqlx::query_scalar("SELECT commit_seq FROM memory_purges WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision>?").bind(ns.as_str()).bind(subject.as_str()).bind(&reference.id).bind(reference.revision).fetch_optional(conn).await.map_err(storage_error)
}
pub(crate) async fn known_identity(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    identity: &CognitiveIdentity,
) -> Result<bool> {
    if identity.kind != CognitiveKind::Memory {
        return Ok(false);
    }
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_purges WHERE namespace_id=? AND subject_id=? AND memory_id=?)").bind(ns.as_str()).bind(subject.as_str()).bind(&identity.id).fetch_one(conn).await.map_err(storage_error)
}
impl WriteTransaction {
    pub(crate) async fn retain_memory_source_tombstones(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        id: &MemoryId,
    ) -> Result<()> {
        let rows=sqlx::query("SELECT revision,observation_id,source FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? ORDER BY revision").bind(ns.as_str()).bind(subject.as_str()).bind(id.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let value = SourceTombstone {
                namespace_id: ns.clone(),
                subject_id: subject.clone(),
                target: RetrievalTarget::Cognition(CognitiveRef::memory(id, row.get("revision"))),
                sources: vec![
                    EvidenceSource::Observation {
                        observation_id: ObservationId(row.get("observation_id")),
                    },
                    EvidenceSource::Media {
                        reference: serde_json::from_str(row.get("source"))?,
                    },
                ],
                recorded_at: self.time().recorded(),
                commit_seq: self.commit_seq(),
                origin: None,
            };
            self.insert_source_tombstone(&value).await?;
        }
        Ok(())
    }
    pub(crate) async fn insert_source_tombstone(&mut self, value: &SourceTombstone) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        value.target.validate()?;
        self.subject(&value.namespace_id, &value.subject_id).await?;
        if !matches!(&value.target,RetrievalTarget::Cognition(reference) if reference.kind==CognitiveKind::Memory)
        {
            return Err(Error::invalid(
                "only Memory permanent erasure currently produces source tombstones",
            ));
        }
        for source in &value.sources {
            source.validate()?;
        }
        sqlx::query("INSERT INTO source_tombstones(namespace_id,subject_id,target_key,body,commit_seq) VALUES(?,?,?,?,?)").bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.target.key()?).bind(serde_json::to_string(value)?).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
