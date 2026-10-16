// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_runtime::DomainTemporalExtent;
use serde::{Deserialize, Serialize};
/// Formal typed facts live on actual Material observations. Text is not parsed
/// into deterministic truth; generated/simulated sources never witness effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CriterionFact {
    Event {
        version: u32,
        event_type: String,
        target: Option<String>,
        activity: Option<String>,
        at: nous_runtime::DomainStamp,
    },
    Measurement {
        version: u32,
        name: String,
        unit: String,
        value: f64,
        error: f64,
        at: nous_runtime::DomainStamp,
    },
}
#[derive(Clone)]
pub struct OfficialCriterionReader {
    pub material: MaterialService,
    pub social: Option<nous_social::SocialService>,
}
#[async_trait::async_trait]
impl CriterionReader for OfficialCriterionReader {
    async fn read_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        criterion: &Criterion,
        interval: &DomainTemporalExtent,
        evidence: &[CognitiveRef],
    ) -> Result<CriterionRead> {
        let mut read = CriterionRead {
            evaluator: match criterion {
                Criterion::EventWitness { .. } => "event_witness/v1",
                Criterion::OwnerState { .. } => "owner_state/v1",
                Criterion::Measurement { .. } => "measurement/v1",
                Criterion::Relation { .. } => "relation/v1",
                Criterion::Semantic { .. } => "semantic/v1",
                _ => return Err(Error::Invalid("composite is not an evaluator leaf".into())),
            }
            .into(),
            samples: vec![],
            complete: false,
            method: "deterministic".into(),
        };
        let (start, end) = extent_bounds(&interval.extent);
        let end = end.ok_or_else(|| {
            Error::Invalid("evaluation requires a bounded captured prefix".into())
        })?;
        match criterion {
            Criterion::OwnerState {
                reference, state, ..
            } => {
                if interval.time_domain_ref != "time:real" {
                    return Ok(read);
                }
                let values =
                    AuthorityStore::cognition_state_history_in(tx, subject, reference, end).await?;
                read.complete =
                    start.is_some_and(|s| values.first().is_some_and(|(at, _)| *at <= s));
                for (at, value) in values {
                    let field = match state.as_str() {
                        "accepted" | "withdrawn" => "acceptance_state",
                        "valid" | "revalidation_required" => "integrity_state",
                        _ => "suppression_state",
                    };
                    let truth = value[field].as_str().map_or(Truth::Unknown, |s| {
                        if s == state {
                            Truth::True
                        } else {
                            Truth::False
                        }
                    });
                    read.samples.push(CriterionSample {
                        at,
                        truth,
                        evidence: vec![reference.clone()],
                    });
                }
            }
            Criterion::Relation {
                reference,
                relation_type,
                degree,
                ..
            } => {
                if interval.time_domain_ref != "time:real" {
                    return Ok(read);
                }
                if let Some(owner) = &self.social {
                    let r = owner
                        .get(subject, reference.uuid().expect("exact"), true)
                        .await?;
                    if let nous_social::SocialContent::SocialRelation(v) = r.content {
                        let kind = serde_json::to_value(v.relation_type)
                            .map_err(|e| Error::Internal(e.to_string()))?;
                        let d = v.degree.as_ref().map(|v| match v {
                            nous_social::RelationDegree::Ordinal(v)
                            | nous_social::RelationDegree::State(v) => v.clone(),
                            nous_social::RelationDegree::Affinity(v) => v.to_string(),
                        });
                        // Exact revision states only its declared valid slice, never an entire past interval.
                        let at = end;
                        let valid = TimePredicate::Point { at }.matches_extent(&v.valid_time);
                        if valid && r.recorded_at <= at {
                            read.samples.push(CriterionSample {
                                at,
                                truth: if kind.as_str() == Some(relation_type)
                                    && degree.as_ref().is_none_or(|x| Some(x) == d.as_ref())
                                {
                                    Truth::True
                                } else {
                                    Truth::False
                                },
                                evidence: vec![reference.clone()],
                            });
                        }
                    }
                }
            }
            Criterion::EventWitness { .. } | Criterion::Measurement { .. } => {
                let ids = evidence
                    .iter()
                    .filter_map(|r| {
                        if let CognitiveRef::Occurrence(id) = r {
                            Some(*id)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                for (id, o) in self
                    .material
                    .occurrences_in(tx, subject, &ids, None)
                    .await?
                {
                    if matches!(
                        o.source_class,
                        SourceClass::Generated | SourceClass::Simulation
                    ) {
                        continue;
                    }
                    let Some(raw) = o.context.get("criterion_fact") else {
                        continue;
                    };
                    let Ok(fact) = serde_json::from_value::<CriterionFact>(raw.clone()) else {
                        continue;
                    };
                    let sample = match (criterion, fact) {
                        (
                            Criterion::EventWitness {
                                event_type,
                                target,
                                activity,
                                ..
                            },
                            CriterionFact::Event {
                                version,
                                event_type: e,
                                target: t,
                                activity: a,
                                at,
                            },
                        ) if version == 1
                            && e == *event_type
                            && target.as_ref().is_none_or(|x| Some(x) == t.as_ref())
                            && activity.as_ref().is_none_or(|x| Some(x) == a.as_ref()) =>
                        {
                            Some((at, Truth::True))
                        }
                        (
                            Criterion::Measurement {
                                name,
                                unit,
                                comparison,
                                value,
                                ..
                            },
                            CriterionFact::Measurement {
                                version,
                                name: n,
                                unit: u,
                                value: measured,
                                error,
                                at,
                            },
                        ) if version == 1
                            && n == *name
                            && u == *unit
                            && measured.is_finite()
                            && error.is_finite()
                            && error >= 0.0 =>
                        {
                            let compare = |x: f64| match comparison {
                                Comparison::Lt => x < *value,
                                Comparison::Le => x <= *value,
                                Comparison::Eq => x == *value,
                                Comparison::Ge => x >= *value,
                                Comparison::Gt => x > *value,
                            };
                            let (lo, hi) = (compare(measured - error), compare(measured + error));
                            let truth = if *comparison == Comparison::Eq
                                && error > 0.0
                                && measured - error <= *value
                                && measured + error >= *value
                            {
                                Truth::Unknown
                            } else if lo && hi {
                                Truth::True
                            } else if !lo && !hi {
                                Truth::False
                            } else {
                                Truth::Unknown
                            };
                            Some((at, truth))
                        }
                        _ => None,
                    };
                    if let Some((at, truth)) = sample
                        && at.time_domain_ref == interval.time_domain_ref
                        && start.is_none_or(|s| at.at >= s)
                        && at.at <= end
                    {
                        read.samples.push(CriterionSample {
                            at: at.at,
                            truth,
                            evidence: vec![CognitiveRef::Occurrence(id)],
                        })
                    }
                }
            }
            Criterion::Semantic { .. } => {
                read.method = "recorded_model_required".into();
            }
            _ => unreachable!(),
        }
        read.samples.sort_by_key(|s| s.at);
        Ok(read)
    }
}
