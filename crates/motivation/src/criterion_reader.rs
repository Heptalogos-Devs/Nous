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
        #[serde(default)]
        value: f64,
        #[serde(default)]
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
            producer: None,
        };
        let mut read = match criterion {
            Criterion::OwnerState { .. } => {
                self.read_state(tx, subject, criterion, interval, read)
                    .await?
            }
            Criterion::Relation { .. } => {
                self.read_relation(subject, criterion, interval, read)
                    .await?
            }
            Criterion::EventWitness { .. } | Criterion::Measurement { .. } => {
                self.read_facts(tx, subject, criterion, interval, evidence, read)
                    .await?
            }
            Criterion::Semantic { .. } => {
                read.method = "recorded_model_required".into();
                read
            }
            _ => unreachable!(),
        };
        read.samples.sort_by_key(|s| s.at);
        Ok(read)
    }
}

impl CriterionFact {
    pub fn to_canonical_json(&self) -> serde_json::Value {
        let stamp = |at: &nous_runtime::DomainStamp| serde_json::json!({"timeDomainRef":at.time_domain_ref,"at":at.at,"recordedAt":at.recorded_at});
        match self {
            Self::Event {
                version,
                event_type,
                target,
                activity,
                at,
            } => {
                serde_json::json!({"event":{"version":version,"eventType":event_type,"target":target,"activity":activity,"at":stamp(at)}})
            }
            Self::Measurement {
                version,
                name,
                unit,
                value,
                error,
                at,
            } => {
                serde_json::json!({"measurement":{"version":version,"name":name,"unit":unit,"value":value,"error":error,"at":stamp(at)}})
            }
        }
    }
    pub fn from_canonical_json(
        raw: &serde_json::Value,
    ) -> std::result::Result<Self, serde_json::Error> {
        // Canonical Protobuf JSON is converted once at the Material transport boundary.
        if raw.as_object().is_none_or(|o| {
            o.len() != 1 || !o.contains_key("event") && !o.contains_key("measurement")
        }) {
            return serde_json::from_value(serde_json::Value::Null);
        }
        let (kind, v) = if raw.get("event").is_some() {
            ("event", &raw["event"])
        } else {
            ("measurement", &raw["measurement"])
        };
        let at = &v["at"];
        let stamp = serde_json::json!({"time_domain_ref":at["timeDomainRef"],"at":at["at"],"recorded_at":at["recordedAt"]});
        let mut value = v.clone();
        if let Some(object) = value.as_object_mut() {
            object.insert("kind".into(), serde_json::json!(kind));
            object.insert("at".into(), stamp);
            if let Some(event_type) = object.remove("eventType") {
                object.insert("event_type".into(), event_type);
            }
        }
        serde_json::from_value(value)
    }
}

impl OfficialCriterionReader {
    async fn read_state(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        criterion: &Criterion,
        interval: &DomainTemporalExtent,
        mut read: CriterionRead,
    ) -> Result<CriterionRead> {
        let Criterion::OwnerState {
            reference, state, ..
        } = criterion
        else {
            unreachable!()
        };
        let (start, end) = extent_bounds(&interval.extent);
        let end = end.ok_or_else(|| Error::Invalid("evaluation needs bounded prefix".into()))?;

        if interval.time_domain_ref != "time:real" {
            return Ok(read);
        }
        let values =
            match AuthorityStore::cognition_state_history_in(tx, subject, reference, end).await {
                Ok(v) => v,
                Err(Error::NotFound(_) | Error::Unavailable(_)) => return Ok(read),
                Err(e) => return Err(e),
            };
        read.complete = start.is_some_and(|s| values.first().is_some_and(|(at, _)| *at <= s));
        for (at, value) in values {
            let field = if matches!(reference, CognitiveRef::WorkContext(_)) {
                "state"
            } else {
                match state.as_str() {
                    "accepted" | "withdrawn" => "acceptance_state",
                    "valid" | "revalidation_required" => "integrity_state",
                    _ => "suppression_state",
                }
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

        Ok(read)
    }
    async fn read_relation(
        &self,
        subject: SubjectId,
        criterion: &Criterion,
        interval: &DomainTemporalExtent,
        mut read: CriterionRead,
    ) -> Result<CriterionRead> {
        let Criterion::Relation {
            reference,
            relation_type,
            degree,
            ..
        } = criterion
        else {
            unreachable!()
        };
        if interval.time_domain_ref != "time:real" {
            return Ok(read);
        }
        let Some(owner) = &self.social else {
            return Ok(read);
        };
        let r = owner
            .get(subject, reference.uuid().expect("exact"), true)
            .await?;
        let nous_social::SocialContent::SocialRelation(v) = r.content else {
            return Ok(read);
        };
        let (_, end) = extent_bounds(&interval.extent);
        let end = end.ok_or_else(|| Error::Invalid("evaluation needs bounded prefix".into()))?;
        let at = if matches!(interval.extent, TemporalExtent::Instant { .. }) {
            end
        } else {
            end - chrono::Duration::microseconds(1)
        };
        if !(TimePredicate::Point { at }).matches_extent(&v.valid_time) || r.recorded_at > at {
            return Ok(read);
        }
        let kind =
            serde_json::to_value(v.relation_type).map_err(|e| Error::Internal(e.to_string()))?;
        let d = v.degree.as_ref().map(|v| match v {
            nous_social::RelationDegree::Ordinal(v) | nous_social::RelationDegree::State(v) => {
                v.clone()
            }
            nous_social::RelationDegree::Affinity(v) => v.to_string(),
        });
        let truth = if kind.as_str() == Some(relation_type)
            && degree.as_ref().is_none_or(|x| Some(x) == d.as_ref())
        {
            Truth::True
        } else {
            Truth::False
        };
        read.samples.push(CriterionSample {
            at,
            truth,
            evidence: vec![reference.clone()],
        });
        Ok(read)
    }
    async fn read_facts(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        criterion: &Criterion,
        interval: &DomainTemporalExtent,
        evidence: &[CognitiveRef],
        mut read: CriterionRead,
    ) -> Result<CriterionRead> {
        let (start, end) = extent_bounds(&interval.extent);
        let end = end.ok_or_else(|| Error::Invalid("evaluation needs bounded prefix".into()))?;

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
            let Ok(fact) = CriterionFact::from_canonical_json(raw) else {
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

        Ok(read)
    }
}
