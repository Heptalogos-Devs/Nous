// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_runtime::{DomainStamp, DomainTemporalExtent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truth {
    True,
    False,
    Unknown,
}
impl Truth {
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionVerdict {
    Satisfied,
    Violated,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionSample {
    pub at: DateTime<Utc>,
    pub truth: Truth,
    pub evidence: Vec<CognitiveRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionRead {
    pub evaluator: String,
    pub samples: Vec<CriterionSample>,
    pub complete: bool,
    pub method: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionEvaluation {
    pub evaluation_id: Uuid,
    pub condition_revision: CognitiveRef,
    pub applicable: Option<bool>,
    pub verdict: ConditionVerdict,
    pub final_conclusion: bool,
    pub evaluated_interval: DomainTemporalExtent,
    pub coverage: String,
    pub evaluator_identity: String,
    pub inputs: Vec<CognitiveRef>,
    pub leaves: Vec<CriterionRead>,
    pub evaluated: DomainStamp,
    pub explanation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateCondition {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub condition_revision: CognitiveRef,
    pub scope: MotivationScope,
    pub source_read: SourceReadScope,
    pub evidence_refs: Vec<CognitiveRef>,
}
/// A concrete owner supplies metadata and its complete authoritative state timeline.
/// Semantic reads are resolved by recorded ModelInvocations, never arbitrary plugin code.
#[async_trait::async_trait]
pub trait CriterionReader: Send + Sync {
    async fn read_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        criterion: &Criterion,
        interval: &DomainTemporalExtent,
        evidence: &[CognitiveRef],
    ) -> Result<CriterionRead>;
}
impl MotivationService {
    pub async fn evaluate(
        &self,
        input: EvaluateCondition,
        reader: &dyn CriterionReader,
    ) -> Result<ConditionEvaluation> {
        self.require_enabled(input.subject).await?;
        if !matches!(
            input.condition_revision,
            CognitiveRef::DesiredConditionRevision(_)
        ) || input.source_read.subject_id != Some(input.subject)
            || input.evidence_refs.len() > 256
        {
            return Err(Error::Invalid(
                "condition requires exact revision and bounded same-Subject SourceRead".into(),
            ));
        }
        let digest = canonical_request_digest("motivation.evaluate", input.subject, &input)?;
        let mut mutation = match self
            .begin(
                input.subject,
                input.operation_id,
                "motivation.evaluate",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(_) => {
                let saved:serde_json::Value=sqlx::query_scalar("SELECT document FROM motivation_evaluations WHERE subject_id=$1 AND operation_id=$2").bind(input.subject.0).bind(input.operation_id.0).fetch_one(self.store.pool()).await.map_err(db)?;
                return serde_json::from_value(saved)
                    .map_err(|e| Error::Infrastructure(e.to_string()));
            }
            MutationStart::Active(m) => m,
        };
        let stamp = self.clocks.capture_in(mutation.tx(), input.subject).await?;
        let allowed = self
            .roots
            .roots_in(mutation.tx(), input.subject, &input.source_read.source_refs)
            .await?;
        let condition_roots = self
            .roots
            .roots_in(
                mutation.tx(),
                input.subject,
                std::slice::from_ref(&input.condition_revision),
            )
            .await;
        if !condition_roots.is_ok_and(|r| {
            !r.is_empty()
                && r.iter()
                    .all(|x| allowed.iter().any(|a| a.root_key == x.root_key))
        }) {
            let result = ConditionEvaluation {
                evaluation_id: Uuid::now_v7(),
                condition_revision: input.condition_revision.clone(),
                applicable: None,
                verdict: ConditionVerdict::Unknown,
                final_conclusion: false,
                evaluated_interval: DomainTemporalExtent {
                    time_domain_ref: stamp.time_domain_ref.clone(),
                    extent: TemporalExtent::Unknown,
                },
                coverage: "gap".into(),
                evaluator_identity: "nous.criterion/v1".into(),
                inputs: vec![],
                leaves: vec![],
                evaluated: stamp.clone(),
                explanation: "Authorized evidence is insufficient".into(),
            };
            save_evaluation(mutation.tx(), &input, &digest, &result).await?;
            mutation
                .commit(
                    "condition_evaluation",
                    Some(&result.evaluation_id.to_string()),
                    None,
                    None,
                )
                .await?;
            return Ok(result);
        }
        let record = self
            .read_in(
                mutation.tx(),
                input.subject,
                input.condition_revision.uuid().expect("exact"),
                true,
            )
            .await?;
        let condition = record
            .content
            .matter()
            .condition
            .as_ref()
            .ok_or_else(|| Error::Invalid("not DesiredCondition".into()))?;
        let mut result = ConditionEvaluation {
            evaluation_id: Uuid::now_v7(),
            condition_revision: input.condition_revision.clone(),
            applicable: Some(true),
            verdict: ConditionVerdict::Unknown,
            final_conclusion: false,
            evaluated_interval: condition.window.clone(),
            coverage: "limited".into(),
            evaluator_identity: "nous.criterion/v1".into(),
            inputs: input.evidence_refs.clone(),
            leaves: vec![],
            evaluated: stamp.clone(),
            explanation: String::new(),
        };
        let requested = std::iter::once(input.condition_revision.clone())
            .chain(input.evidence_refs.iter().cloned())
            .chain(criterion_refs(&condition.expression))
            .collect::<Vec<_>>();
        let roots = self
            .roots
            .roots_in(mutation.tx(), input.subject, &requested)
            .await;
        // Authorization failure never exposes whether a hidden fact or object exists.
        let authorized = roots.is_ok_and(|r| {
            !r.is_empty()
                && r.iter()
                    .all(|x| allowed.iter().any(|a| a.root_key == x.root_key))
        });
        let scoped = scope_matches(record.content.scope(), &input.scope);
        if !authorized || !record.is_current_available() {
            result.applicable = None;
            result.coverage = "gap".into();
            result.explanation = "Authorized evidence is insufficient".into()
        } else if !scoped {
            result.applicable = Some(false);
            result.explanation = "Condition does not apply to this context".into()
        } else if condition.window.time_domain_ref != stamp.time_domain_ref {
            result.applicable = None;
            result.explanation = "Time domains have no declared mapping".into()
        } else {
            let (start, end) = extent_bounds(&condition.window.extent);
            if start.is_some_and(|s| s > stamp.at) {
                result.applicable = Some(false);
                result.explanation = "Condition window has not started".into()
            } else {
                let closed = end.is_some_and(|e| e <= stamp.at);
                let until = end.map_or(stamp.at, |e| e.min(stamp.at));
                result.evaluated_interval.extent = TemporalExtent::Interval {
                    start,
                    end: Some(until),
                };
                let leaves = flatten(&condition.expression);
                for leaf in &leaves {
                    result.leaves.push(
                        reader
                            .read_in(
                                mutation.tx(),
                                input.subject,
                                leaf,
                                &result.evaluated_interval,
                                &input.evidence_refs,
                            )
                            .await?,
                    )
                }
                let mut points = BTreeSet::new();
                if let Some(s) = start {
                    points.insert(s);
                }
                for r in &result.leaves {
                    for s in &r.samples {
                        if start.is_none_or(|lo| s.at >= lo) && s.at <= until {
                            points.insert(s.at);
                        }
                    }
                }
                let values = points
                    .iter()
                    .map(|at| {
                        let mut i = 0;
                        truth_at(&condition.expression, &result.leaves, *at, &mut i)
                    })
                    .collect::<Vec<_>>();
                let complete = start.is_some()
                    && result.leaves.iter().all(|r| r.complete)
                    && !values.is_empty();
                result.coverage = if complete { "complete" } else { "limited" }.into();
                let any_true = values.contains(&Truth::True);
                let any_false = values.contains(&Truth::False);
                let all_true = !values.is_empty() && values.iter().all(|x| *x == Truth::True);
                let all_false = !values.is_empty() && values.iter().all(|x| *x == Truth::False);
                let satisfied = match condition.temporal_form {
                    TemporalForm::Achieve => any_true,
                    TemporalForm::Maintain => closed && complete && all_true,
                    TemporalForm::Avoid => closed && complete && all_false,
                    TemporalForm::Cease => {
                        // Cessation requires positive applicability before a later negative witness.
                        values.first() == Some(&Truth::True)
                            && values.iter().enumerate().any(|(i, t)| {
                                i > 0
                                    && *t == Truth::False
                                    && condition.hold_seconds.is_none_or(|hold| {
                                        let at = *points.iter().nth(i).expect("aligned");
                                        complete
                                            && until.signed_duration_since(at).num_seconds()
                                                >= hold as i64
                                            && values[i..].iter().all(|x| *x == Truth::False)
                                    })
                            })
                    }
                };
                let violated = match condition.temporal_form {
                    TemporalForm::Maintain => any_false,
                    TemporalForm::Avoid => any_true,
                    TemporalForm::Achieve => closed && complete && all_false,
                    TemporalForm::Cease => {
                        closed && complete && values.first() == Some(&Truth::True) && all_true
                    }
                };
                result.verdict = if satisfied {
                    ConditionVerdict::Satisfied
                } else if violated {
                    ConditionVerdict::Violated
                } else {
                    ConditionVerdict::Unknown
                };
                result.final_conclusion = satisfied || violated;
                result.explanation = if result.final_conclusion {
                    "Evidence establishes this temporal condition"
                } else {
                    "Evidence establishes only a bounded prefix; coverage or truth remains unknown"
                }
                .into();
            }
        }
        save_evaluation(mutation.tx(), &input, &digest, &result).await?;
        mutation
            .commit(
                "condition_evaluation",
                Some(&result.evaluation_id.to_string()),
                Some(record.revision_id),
                None,
            )
            .await?;
        Ok(result)
    }
    pub async fn evaluation(&self, subject: SubjectId, id: Uuid) -> Result<ConditionEvaluation> {
        self.require_enabled(subject).await?;
        let value: serde_json::Value = sqlx::query_scalar(
            "SELECT document FROM motivation_evaluations WHERE subject_id=$1 AND evaluation_id=$2",
        )
        .bind(subject.0)
        .bind(id)
        .fetch_optional(self.store.pool())
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("Evaluation unavailable".into()))?;
        serde_json::from_value(value).map_err(|e| Error::Infrastructure(e.to_string()))
    }
}
pub(crate) fn scope_matches(required: &MotivationScope, actual: &MotivationScope) -> bool {
    required
        .entities
        .iter()
        .all(|e| actual.entities.contains(e))
        && required
            .group
            .as_ref()
            .is_none_or(|x| Some(x) == actual.group.as_ref())
        && required
            .channel
            .as_ref()
            .is_none_or(|x| Some(x) == actual.channel.as_ref())
        && required
            .conversation
            .as_ref()
            .is_none_or(|x| Some(x) == actual.conversation.as_ref())
        && required
            .work_context
            .is_none_or(|x| Some(x) == actual.work_context)
        && required
            .domain
            .as_ref()
            .is_none_or(|x| Some(x) == actual.domain.as_ref())
}
pub fn extent_bounds(e: &TemporalExtent) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    match e {
        TemporalExtent::Unknown => (None, None),
        TemporalExtent::Instant { at } => (Some(*at), Some(*at)),
        TemporalExtent::Interval { start, end } => (*start, *end),
    }
}
fn flatten(c: &Criterion) -> Vec<&Criterion> {
    match c {
        Criterion::All { items } | Criterion::Any { items } => {
            items.iter().flat_map(flatten).collect()
        }
        Criterion::Not { item } => flatten(item),
        _ => vec![c],
    }
}
fn truth_at(c: &Criterion, reads: &[CriterionRead], at: DateTime<Utc>, i: &mut usize) -> Truth {
    match c {
        Criterion::Not { item } => truth_at(item, reads, at, i).not(),
        Criterion::All { items } | Criterion::Any { items } => {
            let values = items
                .iter()
                .map(|x| truth_at(x, reads, at, i))
                .collect::<Vec<_>>();
            if matches!(c, Criterion::All { .. }) {
                if values.contains(&Truth::False) {
                    Truth::False
                } else if values.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else {
                    Truth::True
                }
            } else if values.contains(&Truth::True) {
                Truth::True
            } else if values.contains(&Truth::Unknown) {
                Truth::Unknown
            } else {
                Truth::False
            }
        }
        _ => {
            let read = &reads[*i];
            *i += 1;
            if read.complete {
                read.samples
                    .iter()
                    .filter(|s| s.at <= at)
                    .max_by_key(|s| s.at)
                    .map_or(Truth::Unknown, |s| s.truth)
            } else {
                read.samples
                    .iter()
                    .find(|s| s.at == at)
                    .map_or(Truth::Unknown, |s| s.truth)
            }
        }
    }
}

async fn save_evaluation(
    tx: &mut Transaction<'_, Postgres>,
    input: &EvaluateCondition,
    digest: &str,
    result: &ConditionEvaluation,
) -> Result<()> {
    sqlx::query("INSERT INTO motivation_evaluations(evaluation_id,subject_id,condition_revision_id,operation_id,request_digest,document,recorded_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(result.evaluation_id).bind(input.subject.0).bind(input.condition_revision.uuid()).bind(input.operation_id.0).bind(digest).bind(serde_json::to_value(result).map_err(|e|Error::Internal(e.to_string()))?).bind(result.evaluated.recorded_at).execute(&mut **tx).await.map_err(db)?;
    Ok(())
}
fn criterion_refs(c: &Criterion) -> Vec<CognitiveRef> {
    match c {
        Criterion::OwnerState { reference, .. } | Criterion::Relation { reference, .. } => {
            vec![reference.clone()]
        }
        Criterion::Semantic { material_refs, .. } => material_refs.clone(),
        Criterion::All { items } | Criterion::Any { items } => {
            items.iter().flat_map(criterion_refs).collect()
        }
        Criterion::Not { item } => criterion_refs(item),
        _ => vec![],
    }
}
