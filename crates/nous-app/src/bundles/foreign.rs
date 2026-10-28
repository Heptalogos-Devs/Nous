//! External knowledge records become typed cognition with their original records as evidence.
mod assets;
mod images;
mod state;
use super::archive::{self, Loaded};
use crate::bundle_format::{BundleInspection, KnowledgeDoc};
use nous_domain::{media::*, *};
use nous_foundation::clock::Clock;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::OpenOptions, io::Write, path::Path};

pub(super) struct Builder<'a> {
    package: SubjectPackage,
    root: &'a Path,
    memories: BTreeMap<String, CognitiveRef>,
    entities: BTreeMap<String, CognitiveRef>,
    relations: BTreeMap<String, CognitiveRef>,
    episodes: BTreeMap<String, CognitiveRef>,
    clock: OperationTime,
}
pub(super) fn convert(inspection: BundleInspection, root: &Path) -> Result<Loaded> {
    let clock = Clock::operation_snapshot();
    let namespace = NamespaceId::new();
    let subject = SubjectId::new();
    let mut builder = Builder {
        package: SubjectPackage {
            addresses: vec![],
            package: PackageDescriptor {
                id: inspection.manifest.package.id.clone(),
                version: Some(inspection.manifest.package.version.clone()),
                name: inspection.manifest.package.name.clone(),
                description: package_text(&inspection.manifest.package.extra, "description")?,
                author: package_text(&inspection.manifest.package.extra, "author")?,
            },
            content_level: match inspection.manifest.content_level {
                crate::bundle_format::ContentLevel::Knowledge => BundleContentLevel::Knowledge,
                crate::bundle_format::ContentLevel::Full => BundleContentLevel::Full,
            },
            created_at: clock.recorded(),
            watermark: 1,
            subject: Subject {
                id: subject,
                namespace_id: namespace,
                name: inspection.manifest.package.name.clone(),
                capabilities: vec![Capability::Memory],
                revision: 1,
                commit_seq: 1,
            },
            materials: vec![],
            observations: vec![],
            exposures: vec![],
            memories: vec![],
            cognition: vec![],
            source_regions: vec![],
            representations: vec![],
            derived_regions: vec![],
            calls: vec![],
            purges: vec![],
            tombstones: vec![],
            imports: vec![],
            vectors: BundleVectors::default(),
        },
        root,
        memories: BTreeMap::new(),
        entities: BTreeMap::new(),
        relations: BTreeMap::new(),
        episodes: BTreeMap::new(),
        clock,
    };
    for doc in &inspection.knowledge.docs {
        builder.document(doc)?;
    }
    for doc in &inspection.knowledge.docs {
        builder.document_graph(doc)?;
    }
    if let Some(state) = &inspection.state {
        builder.state(state, &inspection.manifest.content_digest)?;
    }
    let mut object_members = match &inspection.images {
        Some(images) => builder.images(images, &inspection.manifest.content_digest)?,
        None => BTreeMap::new(),
    };
    let unaccepted_vector_members = builder.retained_assets(&inspection, &mut object_members)?;
    builder.package.validate()?;
    Ok(Loaded {
        manifest: serde_json::to_value(&inspection.manifest)?,
        package: builder.package,
        digest: inspection.manifest.content_digest,
        members: inspection.members,
        object_members,
        unaccepted_vector_members,
    })
}
fn package_text(values: &BTreeMap<String, Value>, key: &str) -> Result<String> {
    match values.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(Error::invalid(format!("package {key} must be text"))),
    }
}
impl Builder<'_> {
    fn scope(&self) -> (NamespaceId, SubjectId) {
        (
            self.package.subject.namespace_id.clone(),
            self.package.subject.id.clone(),
        )
    }
    fn times(&self) -> CognitiveTimes {
        let point = self.clock.point();
        CognitiveTimes {
            event: None,
            observation: None,
            formation: point.clone(),
            recorded: point,
            validity: None,
        }
    }
    fn evidence(source: EvidenceSource, polarity: EvidencePolarity) -> EvidenceLink {
        EvidenceLink {
            source,
            polarity,
            relation: None,
            explanation: "Imported, attributed source record".into(),
        }
    }
    fn record(
        &mut self,
        key: &str,
        text: &str,
        metadata: Value,
    ) -> Result<(MaterialId, ObservationId)> {
        let bytes = text.as_bytes();
        let hash = format!("{:x}", Sha256::digest(bytes));
        let path = self.root.join(&hash);
        match OpenOptions::new().create_new(true).write(true).open(path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let (namespace_id, subject_id) = self.scope();
        let id = MaterialId::new();
        let observation_id = ObservationId::new();
        self.package.materials.push(MediaOriginal {
            material: Material {
                id: id.clone(),
                namespace_id: namespace_id.clone(),
                content_hash: hash,
                media_type: "text/plain".into(),
                metadata: MediaMetadata::Text,
                byte_len: bytes.len() as i64,
                source: Some(format!("a-memorix:{key}")),
                event_time: None,
                recorded_at: self.package.created_at.clone(),
                commit_seq: 1,
            },
            details: MaterialDetails {
                source_type: Some("a_memorix_bundle".into()),
                tags: vec![],
                metadata,
                structured: None,
            },
            details_redacted: false,
        });
        self.package.observations.push(Observation {
            id: observation_id.clone(),
            namespace_id,
            subject_id,
            material_id: id.clone(),
            observed_at: None,
            origin: None,
            kind: ObservationKind::Import,
            recorded_at: self.package.created_at.clone(),
            commit_seq: 1,
        });
        Ok((id, observation_id))
    }
    fn object(
        &mut self,
        body: CognitiveContent,
        evidence: Vec<EvidenceLink>,
    ) -> Result<CognitiveRef> {
        let (namespace_id, subject_id) = self.scope();
        let object = StructuredObject {
            id: CognitiveId::new(),
            namespace_id,
            subject_id,
            revision: 1,
            body,
            evidence,
            times: self.times(),
            producer_call_id: None,
            recorded_at: self.package.created_at.clone(),
            commit_seq: 1,
            deleted: false,
        };
        object.validate()?;
        let reference = object.reference();
        self.package.cognition.push(object);
        Ok(reference)
    }
    fn document(&mut self, doc: &KnowledgeDoc) -> Result<()> {
        let (material_id, observation_id) = self.record(
            &format!("paragraph:{}", doc.id),
            &doc.passage,
            json!({"external_id":doc.id,"a_memorix":doc.a_memorix,"extra":doc.extra}),
        )?;
        let (namespace_id, subject_id) = self.scope();
        let id = MemoryId::new();
        let mut memory = Memory {
            id: id.clone(),
            namespace_id,
            subject_id,
            revision: 1,
            title: doc
                .passage
                .lines()
                .next()
                .unwrap_or("Imported knowledge")
                .chars()
                .take(120)
                .collect(),
            content: doc.passage.clone(),
            epistemic_status: "reported".into(),
            observation_id,
            material_id: material_id.clone(),
            source: MediaReference::Original { material_id },
            model_call_id: None,
            origin: MemoryOrigin::Imported,
            times: self.times(),
            recorded_at: self.package.created_at.clone(),
            deleted: false,
            lifecycle: MemoryLifecycle::default(),
            commit_seq: 1,
        };
        if let Some(time) = doc
            .a_memorix
            .pointer("/time_meta/event_time")
            .and_then(Value::as_f64)
            .and_then(unix_time)
        {
            memory.times.event = Some(time);
        }
        memory.lifecycle = MemoryLifecycle::new(self.clock.unix_ms());
        self.memories.insert(
            doc.id.clone(),
            CognitiveRef {
                kind: CognitiveKind::Memory,
                id: id.to_string(),
                revision: 1,
            },
        );
        self.package.memories.push(memory);
        Ok(())
    }
    fn entity(&mut self, label: &str, support: &CognitiveRef) -> Result<CognitiveRef> {
        let key = label.trim().to_lowercase();
        if let Some(reference) = self.entities.get(&key) {
            return Ok(reference.clone());
        }
        let reference = self.object(
            CognitiveContent::Entity(EntityContent {
                entity_type: EntityType::Other,
                label: label.trim().into(),
                aliases: vec![],
                description: String::new(),
                identifiers: vec![],
                mentions: vec![EntityMention {
                    target: SemanticTarget::from_reference(RetrievalTarget::Cognition(
                        support.clone(),
                    )),
                    role: "mentioned_in".into(),
                }],
                attributes: vec![],
            }),
            vec![Self::evidence(
                source_ref(support),
                EvidencePolarity::Support,
            )],
        )?;
        self.entities.insert(hash(&key), reference.clone());
        self.entities.insert(key, reference.clone());
        Ok(reference)
    }
    fn document_graph(&mut self, doc: &KnowledgeDoc) -> Result<()> {
        let support = self
            .memories
            .get(&doc.id)
            .cloned()
            .ok_or_else(|| Error::invalid("bundle paragraph mapping missing"))?;
        for label in &doc.extracted_entities {
            self.entity(label, &support)?;
        }
        for triple in &doc.extracted_triples {
            let from = self.entity(&triple[0], &support)?;
            let to = self.entity(&triple[2], &support)?;
            if from.id == to.id {
                continue;
            }
            let key = triple
                .iter()
                .map(|v| v.trim().to_lowercase())
                .collect::<Vec<_>>()
                .join("|");
            if self.relations.contains_key(&key) {
                continue;
            }
            let reference = self.object(
                CognitiveContent::Association(AssociationContent {
                    from,
                    to,
                    relation: triple[1].clone(),
                    polarity: AssociationPolarity::Positive,
                    basis_class: AssociationBasisClass::SourceEvidence,
                    description: "Reported relation in the imported paragraph".into(),
                }),
                vec![Self::evidence(
                    source_ref(&support),
                    EvidencePolarity::Support,
                )],
            )?;
            self.relations.insert(hash(&key), reference.clone());
            self.relations.insert(key, reference);
        }
        Ok(())
    }
}
fn source_ref(reference: &CognitiveRef) -> EvidenceSource {
    if reference.kind == CognitiveKind::Memory {
        EvidenceSource::MemoryRevision {
            memory_id: MemoryId(reference.id.clone()),
            revision: reference.revision,
        }
    } else {
        EvidenceSource::CognitiveRevision {
            reference: reference.clone(),
        }
    }
}
fn unix_time(seconds: f64) -> Option<TimePoint> {
    (seconds.is_finite() && seconds.abs() < (i64::MAX / 1000) as f64).then(|| TimePoint {
        domain: "unix_ms".into(),
        coordinate: (seconds * 1000.0) as i64,
    })
}
fn field(row: &serde_json::Map<String, Value>, key: &str) -> String {
    row.get(key)
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        })
        .unwrap_or_default()
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn reported() -> EpistemicStance {
    EpistemicStance {
        class: EpistemicClass::Reported,
        attribution: Some("A-Memorix imported record".into()),
        confidence: None,
    }
}
