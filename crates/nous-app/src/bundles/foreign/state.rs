use super::*;
use crate::bundle_format::State;
use std::collections::BTreeSet;

type ProfileRecord<'a> = (&'a str, usize, &'a serde_json::Map<String, Value>);

impl Builder<'_> {
    pub(super) fn state(&mut self, state: &State) -> Result<()> {
        // Every exported source record remains independently addressable, including
        // tombstones, external mappings, fact transitions and profile overrides.
        let mut evidence = BTreeMap::new();
        for (table, rows) in &state.tables {
            for (index, row) in rows.iter().enumerate() {
                let (_, observation) = self.record(
                    &format!("{table}/{index}"),
                    &serde_json::to_string(row)?,
                    json!({"table":table,"row_index":index}),
                )?;
                evidence.insert(
                    (table.clone(), index),
                    EvidenceSource::Observation {
                        observation_id: observation,
                    },
                );
            }
        }
        let rows = |table: &str| state.tables.get(table).map(Vec::as_slice).unwrap_or(&[]);
        for row in rows("paragraphs") {
            if let Some(reference) = self.memories.get(&field(row, "hash"))
                && let Some(memory) = self
                    .package
                    .memories
                    .iter_mut()
                    .find(|m| m.id.as_str() == reference.id)
            {
                memory.deleted = flag(row, "is_deleted");
                memory.lifecycle.pinned = flag(row, "is_permanent");
            }
        }
        for (index, row) in rows("entities").iter().enumerate() {
            let name = field(row, "name");
            let key = name.trim().to_lowercase();
            let reference = match self.entities.get(&key) {
                Some(reference) => reference.clone(),
                None => {
                    let reference = self.object(
                        CognitiveContent::Entity(EntityContent {
                            entity_type: EntityType::Other,
                            label: name,
                            aliases: vec![],
                            description: field(row, "description"),
                            identifiers: vec![ExternalEntityIdentity {
                                system: "a_memorix.entity".into(),
                                key: field(row, "hash"),
                            }],
                            mentions: vec![],
                            attributes: vec![],
                        }),
                        vec![Self::evidence(
                            evidence[&("entities".into(), index)].clone(),
                            EvidencePolarity::Support,
                        )],
                    )?;
                    self.entities.insert(key, reference.clone());
                    reference
                }
            };
            if let Some(object) = self
                .package
                .cognition
                .iter_mut()
                .find(|v| v.id.as_str() == reference.id)
            {
                object.deleted = flag(row, "is_deleted");
                let source = evidence[&("entities".into(), index)].clone();
                if !object.evidence.iter().any(|link| link.source == source) {
                    object
                        .evidence
                        .push(Self::evidence(source, EvidencePolarity::Support));
                }
            }
            self.entities.insert(field(row, "hash"), reference);
        }
        for (index, row) in rows("relations").iter().enumerate() {
            let subject = field(row, "subject");
            let object = field(row, "object");
            let predicate = field(row, "predicate");
            let from = self.entities.get(&subject.trim().to_lowercase()).cloned();
            let to = self.entities.get(&object.trim().to_lowercase()).cloned();
            if let (Some(from), Some(to)) = (from, to)
                && from.id != to.id
            {
                let key = [subject, predicate.clone(), object]
                    .iter()
                    .map(|v| v.trim().to_lowercase())
                    .collect::<Vec<_>>()
                    .join("|");
                if let Some(reference) = self.relations.get(&key).cloned() {
                    if let Some(object) = self
                        .package
                        .cognition
                        .iter_mut()
                        .find(|v| v.id.as_str() == reference.id)
                    {
                        object.deleted = flag(row, "is_inactive");
                        object.evidence.push(Self::evidence(
                            evidence[&("relations".into(), index)].clone(),
                            EvidencePolarity::Support,
                        ));
                    }
                    self.relations.insert(field(row, "hash"), reference);
                    continue;
                }
                let reference = self.object(
                    CognitiveContent::Association(AssociationContent {
                        from,
                        to,
                        relation: predicate,
                        polarity: if field(row, "polarity") == "negative" {
                            AssociationPolarity::Negative
                        } else {
                            AssociationPolarity::Positive
                        },
                        basis_class: AssociationBasisClass::SourceEvidence,
                        description: field(row, "description"),
                    }),
                    vec![Self::evidence(
                        evidence[&("relations".into(), index)].clone(),
                        EvidencePolarity::Support,
                    )],
                )?;
                if let Some(value) = self.package.cognition.last_mut() {
                    value.deleted = flag(row, "is_inactive");
                }
                self.relations.insert(field(row, "hash"), reference.clone());
                self.relations.insert(key, reference);
            }
        }
        for (index, row) in rows("episodes").iter().enumerate() {
            let id = field(row, "episode_id");
            let mut links = rows("episode_paragraphs")
                .iter()
                .filter(|r| field(r, "episode_id") == id)
                .collect::<Vec<_>>();
            links.sort_by_key(|r| r.get("position").and_then(Value::as_i64).unwrap_or(0));
            let mut members = links
                .into_iter()
                .filter_map(|r| self.memories.get(&field(r, "paragraph_hash")))
                .map(source_ref)
                .collect::<Vec<_>>();
            if members.is_empty() {
                members.push(evidence[&("episodes".into(), index)].clone());
            }
            let mut seen = BTreeSet::new();
            members.retain(|m| seen.insert(m.clone()));
            let period = TimeRange {
                domain: "unix_ms".into(),
                start: row
                    .get("event_time_start")
                    .and_then(Value::as_f64)
                    .and_then(unix_time)
                    .map(|t| t.coordinate),
                end: row
                    .get("event_time_end")
                    .and_then(Value::as_f64)
                    .and_then(unix_time)
                    .map(|t| t.coordinate),
            };
            let reference = self.object(
                CognitiveContent::Episode(EpisodeContent {
                    title: field(row, "title"),
                    track: nonempty(field(row, "source"), "a_memorix"),
                    experience: period,
                    parent: None,
                    members: members
                        .into_iter()
                        .enumerate()
                        .map(|(index, source)| EpisodeMember {
                            source,
                            role: "reported_event".into(),
                            ordinal: index as u32,
                        })
                        .collect(),
                    boundary_explanation: nonempty(
                        field(row, "summary"),
                        "Imported episode boundary",
                    ),
                    lineage: vec![],
                }),
                vec![Self::evidence(
                    evidence[&("episodes".into(), index)].clone(),
                    EvidencePolarity::Support,
                )],
            )?;
            self.episodes.insert(id, reference);
        }
        for (index, row) in rows("fact_claims").iter().enumerate() {
            let id = field(row, "claim_id");
            let mut support = vec![evidence[&("fact_claims".into(), index)].clone()];
            let mut contradict = vec![];
            let mut context = vec![];
            for source in rows("fact_evidence")
                .iter()
                .filter(|r| field(r, "claim_id") == id)
            {
                let source_id = field(source, "evidence_id");
                let stance = field(source, "stance");
                let source = match field(source, "evidence_type").as_str() {
                    "paragraph" => self.memories.get(&source_id).map(source_ref),
                    "episode" => self.episodes.get(&source_id).map(source_ref),
                    _ => None,
                };
                if let Some(source) = source {
                    match stance.as_str() {
                        "against" | "contradict" | "refute" => contradict.push(source),
                        "context" => context.push(source),
                        _ => support.push(source),
                    }
                }
            }
            let mut seen = BTreeSet::new();
            support.retain(|v| seen.insert(v.clone()));
            seen.clear();
            contradict.retain(|v| seen.insert(v.clone()));
            seen.clear();
            context.retain(|v| seen.insert(v.clone()));
            self.object(
                CognitiveContent::Schema(SchemaContent {
                    title: nonempty(field(row, "fact_key"), "Imported fact claim"),
                    claim: nonempty(field(row, "value_text"), &serde_json::to_string(row)?),
                    scope: SchemaScope {
                        description: format!(
                            "{}:{}",
                            field(row, "scope_type"),
                            field(row, "scope_id")
                        ),
                        about: vec![],
                        concepts: vec![],
                        validity: validity(row),
                    },
                    boundary: format!(
                        "Imported authority={}, stability={}, polarity={}, cardinality={}",
                        field(row, "authority"),
                        field(row, "stability"),
                        field(row, "polarity"),
                        field(row, "cardinality")
                    ),
                    formation_mode: FormationMode::Grounded,
                    epistemic: reported(),
                    support,
                    counterexamples: contradict,
                    boundary_cases: context,
                    lineage: vec![],
                }),
                vec![],
            )?;
            if let Some(value) = self.package.cognition.last_mut() {
                value.deleted = field(row, "status") != "active";
            }
        }
        self.profiles(state, &evidence)?;
        Ok(())
    }
    fn profiles(
        &mut self,
        state: &State,
        evidence: &BTreeMap<(String, usize), EvidenceSource>,
    ) -> Result<()> {
        let mut people: BTreeMap<String, Vec<ProfileRecord<'_>>> = BTreeMap::new();
        for table in [
            "person_profile_snapshots",
            "person_profile_overrides",
            "person_profile_alias_overrides",
        ] {
            for (index, row) in state.tables.get(table).into_iter().flatten().enumerate() {
                people
                    .entry(field(row, "person_id"))
                    .or_default()
                    .push((table, index, row));
            }
        }
        for (person, mut records) in people {
            records.sort_by_key(|(table, _, row)| {
                (
                    match *table {
                        "person_profile_snapshots" => 0,
                        "person_profile_overrides" => 1,
                        _ => 2,
                    },
                    row.get("profile_version")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                )
            });
            let id = CognitiveId::new();
            let mut previous: Option<EntityContent> = None;
            for (ordinal, (table, index, row)) in records.into_iter().enumerate() {
                let revision = i64::try_from(ordinal + 1).map_err(|error| {
                    Error::invalid("profile revision exceeds its bound").with_cause(&error)
                })?;
                let source = evidence[&(table.to_string(), index)].clone();
                let mut entity = previous.clone().unwrap_or(EntityContent {
                    entity_type: EntityType::Person,
                    label: person.clone(),
                    aliases: vec![],
                    description: String::new(),
                    identifiers: vec![ExternalEntityIdentity {
                        system: "a_memorix.person".into(),
                        key: person.clone(),
                    }],
                    mentions: vec![],
                    attributes: vec![],
                });
                if table == "person_profile_snapshots" {
                    entity.description = field(row, "profile_text");
                }
                if table == "person_profile_overrides" {
                    entity.description = field(row, "override_text");
                }
                if let Some(raw) = row.get("aliases_json") {
                    let aliases = if let Some(text) = raw.as_str() {
                        serde_json::from_str::<Value>(text).ok()
                    } else {
                        Some(raw.clone())
                    };
                    if let Some(Value::Array(values)) = aliases {
                        let mut seen = BTreeSet::from([entity.label.trim().to_lowercase()]);
                        entity.aliases = values
                            .into_iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .filter(|v| {
                                !v.trim().is_empty() && seen.insert(v.trim().to_lowercase())
                            })
                            .collect();
                    }
                }
                let text = match table {
                    "person_profile_snapshots" => field(row, "profile_text"),
                    "person_profile_overrides" => field(row, "override_text"),
                    _ => serde_json::to_string(row)?,
                };
                entity
                    .attributes
                    .retain(|attribute| attribute.predicate != table);
                if !text.trim().is_empty() {
                    entity.attributes.push(EntityAttribute {
                        predicate: table.into(),
                        value: text,
                        epistemic: reported(),
                        validity: None,
                        evidence: vec![Self::evidence(source.clone(), EvidencePolarity::Support)],
                    });
                }
                let (namespace_id, subject_id) = self.scope();
                let value = StructuredObject {
                    id: id.clone(),
                    namespace_id,
                    subject_id,
                    revision,
                    body: CognitiveContent::Entity(entity.clone()),
                    evidence: vec![Self::evidence(source, EvidencePolarity::Support)],
                    times: self.times(),
                    producer_call_id: None,
                    recorded_at: self.package.created_at.clone(),
                    commit_seq: 1,
                    deleted: false,
                };
                value.validate()?;
                self.package.cognition.push(value);
                previous = Some(entity);
            }
        }
        Ok(())
    }
}
fn nonempty(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.into()
    } else {
        value
    }
}
fn validity(row: &serde_json::Map<String, Value>) -> Option<TimeRange> {
    let start = row
        .get("valid_from")
        .and_then(Value::as_f64)
        .and_then(unix_time)
        .map(|t| t.coordinate);
    let end = row
        .get("valid_to")
        .and_then(Value::as_f64)
        .and_then(unix_time)
        .map(|t| t.coordinate);
    (start.is_some() || end.is_some()).then_some(TimeRange {
        domain: "unix_ms".into(),
        start,
        end,
    })
}
fn flag(row: &serde_json::Map<String, Value>, key: &str) -> bool {
    row.get(key).is_some_and(|v| {
        v.as_bool()
            .unwrap_or_else(|| v.as_i64().is_some_and(|v| v != 0))
    })
}
