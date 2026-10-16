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
    pub producer: Option<ProducerSignature>,
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
            .read_scope_in(mutation.tx(), input.subject, &input.source_read)
            .await?;
        if !self
            .references_allowed_in(
                mutation.tx(),
                &allowed,
                &[(input.condition_revision.clone(), false)],
            )
            .await?
        {
            let result = empty_evaluation(input.condition_revision.clone(), stamp.clone(), None);
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
        let result = self
            .evaluate_record(mutation.tx(), &input, &record, &stamp, reader, &allowed)
            .await?;
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
    async fn evaluate_record(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        input: &EvaluateCondition,
        record: &MotivationRecord,
        stamp: &DomainStamp,
        reader: &dyn CriterionReader,
        allowed: &SourceReadView,
    ) -> Result<ConditionEvaluation> {
        let condition = record
            .content
            .matter()
            .condition
            .as_ref()
            .ok_or_else(|| Error::Invalid("not DesiredCondition".into()))?;
        let mut result = empty_evaluation(
            input.condition_revision.clone(),
            stamp.clone(),
            Some(condition.window.clone()),
        );
        result.applicable = Some(true);
        result.coverage = "limited".into();
        let requested = std::iter::once(input.condition_revision.clone())
            .chain(input.evidence_refs.iter().cloned())
            .chain(criterion_refs(&condition.expression))
            .collect::<Vec<_>>();
        let accesses = input
            .evidence_refs
            .iter()
            .cloned()
            .map(|r| (r, false))
            .chain(criterion_accesses(&condition.expression))
            .collect::<Vec<_>>();
        let authorized = self.references_allowed_in(tx, allowed, &accesses).await?;
        result.inputs = requested.clone();
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
                                tx,
                                input.subject,
                                leaf,
                                &result.evaluated_interval,
                                &input.evidence_refs,
                            )
                            .await?,
                    )
                }
                fold_temporal(condition, stamp, &mut result);
            }
        }
        Ok(result)
    }
    pub async fn evaluations(
        &self,
        subject: SubjectId,
        condition: CognitiveRef,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<(Vec<ConditionEvaluation>, bool)> {
        self.require_enabled(subject).await?;
        if !matches!(condition, CognitiveRef::DesiredConditionRevision(_))
            || limit == 0
            || limit > 100
        {
            return Err(Error::Invalid(
                "Evaluation history requires an exact condition and bounded page".into(),
            ));
        }
        let rows:Vec<serde_json::Value>=sqlx::query_scalar("SELECT document FROM motivation_evaluations WHERE subject_id=$1 AND condition_revision_id=$2 AND ($3::uuid IS NULL OR evaluation_id>$3) ORDER BY evaluation_id LIMIT $4").bind(subject.0).bind(condition.uuid()).bind(after).bind((limit+1) as i64).fetch_all(self.store.pool()).await.map_err(db)?;
        let more = rows.len() > limit;
        Ok((
            rows.into_iter()
                .take(limit)
                .map(|v| {
                    serde_json::from_value(v).map_err(|e| Error::Infrastructure(e.to_string()))
                })
                .collect::<Result<Vec<_>>>()?,
            more,
        ))
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
    for producer in result.leaves.iter().filter_map(|l| l.producer.as_ref()) {
        AuthorityStore::register_producer_in(tx, producer).await?;
    }
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

fn fold_temporal(
    condition: &DesiredCondition,
    stamp: &DomainStamp,
    result: &mut ConditionEvaluation,
) {
    let (start, end) = extent_bounds(&condition.window.extent);
    let until = end.map_or(stamp.at, |e| e.min(stamp.at));
    let closed = end.is_some_and(|e| e <= stamp.at);
    let mut points = BTreeSet::new();
    if let Some(s) = start {
        points.insert(s);
    }
    for r in &result.leaves {
        for s in &r.samples {
            if start.is_none_or(|lo| s.at >= lo)
                && (s.at < until
                    || matches!(condition.window.extent, TemporalExtent::Instant{at} if at==s.at))
            {
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
    let complete =
        start.is_some() && result.leaves.iter().all(|r| r.complete) && !values.is_empty();
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
                                && until.signed_duration_since(at).num_seconds() >= hold as i64
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

fn empty_evaluation(
    reference: CognitiveRef,
    stamp: DomainStamp,
    interval: Option<DomainTemporalExtent>,
) -> ConditionEvaluation {
    ConditionEvaluation {
        evaluation_id: Uuid::now_v7(),
        condition_revision: reference,
        applicable: None,
        verdict: ConditionVerdict::Unknown,
        final_conclusion: false,
        evaluated_interval: interval.unwrap_or_else(|| DomainTemporalExtent {
            time_domain_ref: stamp.time_domain_ref.clone(),
            extent: TemporalExtent::Unknown,
        }),
        coverage: "gap".into(),
        evaluator_identity: "nous.criterion/v1".into(),
        inputs: vec![],
        leaves: vec![],
        evaluated: stamp,
        explanation: "Authorized evidence is insufficient".into(),
    }
}
