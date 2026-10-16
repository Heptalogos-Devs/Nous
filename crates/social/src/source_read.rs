// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SocialService {
    pub(crate) async fn index_source_roots(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        revision: Uuid,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        let mut refs = Vec::new();
        for b in basis {
            match b {
                RevisionBasis::Evidence(e) => refs.push(match &e.locator {
                    EvidenceLocator::WholeOccurrence => CognitiveRef::Occurrence(e.occurrence_id),
                    _ => b.evidence_handle().reference,
                }),
                RevisionBasis::CognitionDependency(d) => refs.push(d.target_revision.clone()),
                RevisionBasis::Seed(s) => {
                    refs.push(CognitiveRef::CognitiveSeedVersion(s.seed_version_id))
                }
            }
        }
        let roots = self.roots.roots_in(tx, subject, &refs).await?;
        if roots.is_empty() {
            return Err(Error::Invalid(
                "Social source roots cannot be resolved".into(),
            ));
        }
        for root in roots {
            sqlx::query("INSERT INTO social_source_roots(revision_id,root_key,certainty) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(revision).bind(root.root_key).bind(match root.certainty{EvidenceRootCertainty::Known=>"known",EvidenceRootCertainty::OccurrenceOnly=>"occurrence_only"}).execute(&mut **tx).await.map_err(db)?;
        }
        Ok(())
    }
}

impl SocialService {
    /// Rebuild a bounded chronological page of the permission index from immutable
    /// owner basis and append-only evidence. This grants no new source authority.
    pub async fn rebuild_source_index(
        &self,
        subject: SubjectId,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Option<Uuid>> {
        self.require_enabled(subject).await?;
        if limit == 0 || limit > 256 {
            return Err(Error::Invalid("invalid source-index rebuild page".into()));
        }
        let mut tx = self.store.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("social-authority:{}", subject.0))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let ids:Vec<Uuid>=sqlx::query_scalar("SELECT revision_id FROM social_revisions WHERE subject_id=$1 AND ($2::uuid IS NULL OR revision_id>$2) ORDER BY revision_id LIMIT $3").bind(subject.0).bind(after).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit;
        let ids = &ids[..ids.len().min(limit)];
        let records = self.read_many_in(&mut tx, subject, ids, true).await?;
        for record in records {
            sqlx::query("DELETE FROM social_source_roots WHERE revision_id=$1")
                .bind(record.revision_id)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            self.index_source_roots(&mut tx, subject, record.revision_id, record.content.basis())
                .await?;
            for evidence in record.evidence {
                self.index_source_roots(
                    &mut tx,
                    subject,
                    record.revision_id,
                    std::slice::from_ref(&evidence.basis),
                )
                .await?;
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(if more { ids.last().copied() } else { None })
    }
}
