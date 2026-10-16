//! Context consumes bounded content and exact support in one captured owner read.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_runtime::ContextSource;

impl MemoryService {
    #[expect(
        clippy::too_many_lines,
        reason = "captured source authorization and bounded owner materialization stay in one read view"
    )]
    pub async fn context_sources(
        &self,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        policy: &AccessibilityPolicy,
        now: DateTime<Utc>,
        max_bytes: usize,
        source_read: Option<&SourceReadView>,
    ) -> Result<Vec<Result<ContextSource>>> {
        let mut tx = self.store.read_view().await?;
        let mut admitted = Vec::new();
        let mut indices = Vec::new();
        for (index, reference) in references.iter().enumerate() {
            if source_read.is_none()
                || AuthorityStore::source_read_allowed_in(
                    &mut tx,
                    source_read.expect("checked read"),
                    &reference.0,
                )
                .await?
            {
                admitted.push(reference.clone());
                indices.push(index);
            }
        }
        let contents = self
            .content_in(
                &mut tx,
                subject,
                &admitted,
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
        let mut points = std::collections::HashMap::<Uuid, Vec<Option<String>>>::new();
        if max_bytes > 0 && !journals.is_empty() {
            for row in sqlx::query("SELECT journal_revision_id,CASE WHEN octet_length(text)<=$2 THEN text END text FROM journal_revision_points WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ordinal")
                .bind(&journals).bind(max_bytes as i32).fetch_all(&mut *tx).await.map_err(db)? {
                let text: Option<String> = row.try_get("text").map_err(db)?;
                points.entry(row.try_get("journal_revision_id").map_err(db)?).or_default().push(text);
            }
        }
        let results: Vec<_> = contents
            .into_iter()
            .map(|content| {
                content.and_then(|content| {
                    if max_bytes > 0
                        && (!content.complete
                            || points
                                .get(&content.revision)
                                .is_some_and(|values| values.iter().any(Option::is_none)))
                    {
                        return Err(complete_unit_error());
                    }
                    let mut text = content.text;
                    if let Some(scope) = content.scope {
                        text.push('\n');
                        text.push_str(&scope);
                    }
                    if let Some(boundary) = content.boundary {
                        text.push('\n');
                        text.push_str(&boundary);
                    }
                    for point in points
                        .get(&content.revision)
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
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
                    if max_bytes > 0 && text.len() > max_bytes {
                        return Err(complete_unit_error());
                    }
                    Ok(ContextSource {
                        usage: "task_evidence",
                        required: false,
                        reference: content.reference,
                        source_revision: Some(content.revision.to_string()),
                        text: (max_bytes > 0).then_some(text),
                        authority: AuthorityClass::SubjectCognition,
                        evidence,
                    })
                })
            })
            .collect();
        tx.commit().await.map_err(db)?;
        let mut ordered = (0..references.len())
            .map(|_| {
                Err(Error::Unavailable(
                    "source read denied (unknown/cyclic/over-budget lineage withheld)".into(),
                ))
            })
            .collect::<Vec<_>>();
        for (index, result) in indices.into_iter().zip(results) {
            ordered[index] = result;
        }
        Ok(ordered)
    }
}

fn complete_unit_error() -> Error {
    Error::Unavailable("complete cognitive unit exceeds projection budget".into())
}
