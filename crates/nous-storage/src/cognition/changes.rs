use crate::cognition::management::purge_row;
use crate::cognition::memory::memory_row;
use crate::{Database, storage_error};
use nous_domain::PurgedMemory;
use nous_domain::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatch {
    pub through_commit_seq: i64,
    pub memories: Vec<Memory>,
    pub purged: Vec<PurgedMemory>,
    pub has_more: bool,
}

impl Database {
    pub async fn memory_changes(&self, after_seq: i64, commit_limit: usize) -> Result<MemoryBatch> {
        self.changes(None, after_seq, commit_limit).await
    }

    pub async fn subject_memory_changes(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        after_seq: i64,
        commit_limit: usize,
    ) -> Result<MemoryBatch> {
        self.changes(Some((namespace, subject)), after_seq, commit_limit)
            .await
    }

    async fn changes(
        &self,
        scope: Option<(&NamespaceId, &SubjectId)>,
        after_seq: i64,
        commit_limit: usize,
    ) -> Result<MemoryBatch> {
        let limit = i64::try_from(commit_limit)
            .ok()
            .and_then(|v| v.checked_add(1))
            .filter(|_| commit_limit > 0 && after_seq >= 0)
            .ok_or_else(|| {
                Error::invalid(
                    "change cursor must be nonnegative and commit limit must be positive",
                )
            })?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // This read establishes the WAL snapshot used by both pagination and
        // materialization; a concurrent commit is visible on the next batch.
        let upper: i64 =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if after_seq > upper {
            return Err(Error::new(
                ErrorCode::Conflict,
                "change cursor is ahead of authority",
            ));
        }
        let (namespace, subject) = scope
            .map(|(ns, subject)| (Some(ns.as_str()), Some(subject.as_str())))
            .unwrap_or((None, None));
        let sequences: Vec<i64> = sqlx::query_scalar("WITH changes AS (SELECT commit_seq,namespace_id,subject_id FROM memory_revisions UNION ALL SELECT commit_seq,namespace_id,subject_id FROM memory_purges) SELECT DISTINCT commit_seq FROM changes WHERE commit_seq>? AND commit_seq<=? AND (? IS NULL OR namespace_id=?) AND (? IS NULL OR subject_id=?) ORDER BY commit_seq LIMIT ?")
            .bind(after_seq).bind(upper).bind(namespace).bind(namespace).bind(subject).bind(subject).bind(limit)
            .fetch_all(&mut *tx).await.map_err(storage_error)?;
        let has_more = sequences.len() > commit_limit;
        let through = if has_more {
            sequences[commit_limit - 1]
        } else {
            upper
        };
        let rows = sqlx::query("SELECT r.* FROM memory_revisions r WHERE r.commit_seq>? AND r.commit_seq<=? AND (? IS NULL OR r.namespace_id=?) AND (? IS NULL OR r.subject_id=?) AND r.revision=(SELECT max(latest.revision) FROM memory_revisions latest WHERE latest.memory_id=r.memory_id AND latest.commit_seq<=?) ORDER BY r.commit_seq,r.memory_id")
            .bind(after_seq).bind(through).bind(namespace).bind(namespace).bind(subject).bind(subject).bind(through)
            .fetch_all(&mut *tx).await.map_err(storage_error)?;
        let memories = rows
            .into_iter()
            .map(memory_row)
            .collect::<Result<Vec<_>>>()?;
        let purged = sqlx::query("SELECT * FROM memory_purges WHERE commit_seq>? AND commit_seq<=? AND (? IS NULL OR namespace_id=?) AND (? IS NULL OR subject_id=?) ORDER BY commit_seq,memory_id")
            .bind(after_seq).bind(through).bind(namespace).bind(namespace).bind(subject).bind(subject)
            .fetch_all(&mut *tx).await.map_err(storage_error)?.into_iter().map(purge_row).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(MemoryBatch {
            through_commit_seq: through,
            memories,
            purged,
            has_more,
        })
    }
}
