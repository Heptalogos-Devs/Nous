// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
#[derive(Debug, Clone)]
pub struct SemanticInput {
    pub key: String,
    pub question: String,
    pub references: Vec<CognitiveRef>,
}
#[derive(Debug, Clone)]
pub struct SemanticPreparation {
    pub request_digest: String,
    pub leaves: Vec<SemanticInput>,
    pub dependencies: Vec<CognitiveRef>,
    pub evaluated: nous_runtime::DomainStamp,
}
impl MotivationService {
    /// Fixed source-authorized leaf catalog. Host materialization happens only after
    /// these exact roots are admitted; it does not choose new references via a model.
    pub async fn prepare_semantic(&self, input: &EvaluateCondition) -> Result<SemanticPreparation> {
        self.require_enabled(input.subject).await?;
        let request_digest = canonical_request_digest("motivation.evaluate", input.subject, input)?;
        let snapshot = self.clocks.read(input.subject).await?;
        let mut result = SemanticPreparation {
            request_digest,
            leaves: vec![],
            dependencies: vec![input.condition_revision.clone()],
            evaluated: nous_runtime::DomainStamp {
                time_domain_ref: snapshot.time_domain_ref,
                at: snapshot.world_now,
                recorded_at: snapshot.recorded_at,
            },
        };
        if input.source_read.subject_id != Some(input.subject) {
            return Err(Error::Invalid("same-Subject SourceRead required".into()));
        }
        let mut tx = self.store.read_view().await?;
        let allowed = self
            .roots
            .roots_in(&mut tx, input.subject, &input.source_read.source_refs)
            .await?;
        let permitted = |roots: &std::collections::BTreeSet<EvidenceRoot>| {
            !roots.is_empty()
                && roots
                    .iter()
                    .all(|r| allowed.iter().any(|a| a.root_key == r.root_key))
        };
        if !self
            .roots
            .roots_in(
                &mut tx,
                input.subject,
                std::slice::from_ref(&input.condition_revision),
            )
            .await
            .is_ok_and(|r| permitted(&r))
        {
            return Ok(result);
        }
        let record = self
            .read_in(
                &mut tx,
                input.subject,
                input
                    .condition_revision
                    .uuid()
                    .ok_or_else(|| Error::Invalid("exact condition required".into()))?,
                true,
            )
            .await?;
        if !record.is_current_available()
            || !super::evaluator::scope_matches(record.content.scope(), &input.scope)
        {
            return Ok(result);
        }
        let Some(condition) = &record.content.matter().condition else {
            return Err(Error::Invalid("not a DesiredCondition".into()));
        };
        if condition.window.time_domain_ref != result.evaluated.time_domain_ref {
            return Ok(result);
        }
        let required = criterion_references(&condition.expression)
            .into_iter()
            .chain(input.evidence_refs.clone())
            .collect::<Vec<_>>();
        if !required.is_empty()
            && !self
                .roots
                .roots_in(&mut tx, input.subject, &required)
                .await
                .is_ok_and(|r| permitted(&r))
        {
            return Ok(result);
        }
        fn collect(subject: SubjectId, c: &Criterion, out: &mut Vec<SemanticInput>) -> Result<()> {
            match c {
                Criterion::All { items } | Criterion::Any { items } => {
                    for c in items {
                        collect(subject, c, out)?;
                    }
                }
                Criterion::Not { item } => collect(subject, item, out)?,
                Criterion::Semantic {
                    question,
                    material_refs,
                    ..
                } => out.push(SemanticInput {
                    key: canonical_request_digest("motivation.semantic-leaf", subject, c)?,
                    question: question.clone(),
                    references: material_refs.clone(),
                }),
                _ => {}
            }
            Ok(())
        }
        collect(input.subject, &condition.expression, &mut result.leaves)?;
        // Duplicate identical leaves share one recorded evaluation, never independent support.
        result.leaves.sort_by(|a, b| a.key.cmp(&b.key));
        result.leaves.dedup_by(|a, b| a.key == b.key);
        result.dependencies.extend(required);
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
}
