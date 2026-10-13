//! Context consumes bounded content and exact support in one captured owner read.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_runtime::ContextSource;

impl MemoryService {
    pub async fn context_sources(
        &self,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        policy: &AccessibilityPolicy,
        now: DateTime<Utc>,
        max_bytes: usize,
    ) -> Result<Vec<Result<ContextSource>>> {
        let mut tx = self.store.read_view().await?;
        let contents = self
            .content_in(
                &mut tx,
                subject,
                references,
                policy,
                None,
                now,
                CognitiveEffort::Normal,
                max_bytes,
            )
            .await?;
        let exact = contents
            .iter()
            .filter_map(|c| c.as_ref().ok())
            .filter(|c| !matches!(c.reference, CognitiveRef::Tag(_)))
            .map(|c| c.reference.clone())
            .collect::<Vec<_>>();
        let mut supports = self
            .exact_supports_in(&mut tx, subject, &exact, None)
            .await?;
        let journals = exact
            .iter()
            .filter_map(|r| {
                if let CognitiveRef::JournalRevision(id) = r {
                    Some(id.0)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let mut sources = std::collections::HashMap::<Uuid, Vec<CognitiveRef>>::new();
        for row in sqlx::query("SELECT journal_revision_id,ref_kind,ref_value FROM journal_revision_sources WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ref_kind,ref_value")
            .bind(&journals).fetch_all(&mut *tx).await.map_err(db)? {
            sources.entry(row.try_get("journal_revision_id").map_err(db)?).or_default().push(parse_reference(
                &row.try_get::<String,_>("ref_kind").map_err(db)?, &row.try_get::<String,_>("ref_value").map_err(db)?)?);
        }
        let mut points = std::collections::HashMap::<Uuid, Vec<String>>::new();
        if max_bytes > 0 && !journals.is_empty() {
            for row in sqlx::query("SELECT journal_revision_id,left(text,$2) text FROM journal_revision_points WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ordinal")
                .bind(&journals).bind(max_bytes as i32).fetch_all(&mut *tx).await.map_err(db)? {
                points.entry(row.try_get("journal_revision_id").map_err(db)?).or_default().push(row.try_get("text").map_err(db)?);
            }
        }
        let results = contents
            .into_iter()
            .map(|content| {
                content.map(|content| {
                    let mut text = content.text;
                    if let Some(scope) = content.scope {
                        text.push('\n');
                        text.push_str(&scope);
                    }
                    for point in points.get(&content.revision).into_iter().flatten() {
                        text.push('\n');
                        text.push_str(point);
                    }
                    let mut evidence =
                        supports
                            .get_mut(&content.reference)
                            .map_or_else(Vec::new, |support| {
                                support
                                    .basis
                                    .iter()
                                    .map(RevisionBasis::evidence_handle)
                                    .collect()
                            });
                    evidence.extend(
                        sources
                            .get(&content.revision)
                            .into_iter()
                            .flatten()
                            .cloned()
                            .map(|reference| EvidenceHandle {
                                reference,
                                epistemic_relation: None,
                                basis_role: "source".into(),
                            }),
                    );
                    text.truncate(text.floor_char_boundary(max_bytes));
                    ContextSource {
                        reference: content.reference,
                        source_revision: Some(content.revision.to_string()),
                        text: (max_bytes > 0).then_some(text),
                        authority: AuthorityClass::SubjectCognition,
                        evidence,
                    }
                })
            })
            .collect();
        tx.commit().await.map_err(db)?;
        Ok(results)
    }
}
