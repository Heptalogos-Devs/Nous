use super::*;
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;

impl App {
    pub(super) async fn current_resident_target(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        target: &RetrievalTarget,
    ) -> Result<Option<RetrievalTarget>> {
        let value = match target {
            RetrievalTarget::Cognition(reference) if reference.kind == CognitiveKind::Memory => {
                let value = self
                    .get_memory(ctx, &MemoryId(reference.id.clone()))
                    .await?;
                if value.subject_id != *subject {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "resident belongs to another subject",
                    ));
                }
                value.is_active().then(|| {
                    RetrievalTarget::Cognition(CognitiveRef::memory(&value.id, value.revision))
                })
            }
            RetrievalTarget::Cognition(reference) => {
                let value = self
                    .get_structured(ctx, &CognitiveId(reference.id.clone()), None)
                    .await?;
                if value.subject_id != *subject {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "resident belongs to another subject",
                    ));
                }
                value
                    .is_active()
                    .then(|| RetrievalTarget::Cognition(value.reference()))
            }
            RetrievalTarget::Media(_) => Some(target.clone()),
        };
        Ok(value)
    }
    pub(super) async fn context_segment(
        &self,
        ctx: &CallContext,
        ns: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
        required: bool,
    ) -> Result<ContextSegment> {
        target.validate()?;
        ctx.require_subject(ns, subject)?;
        let (text, authority, provenance, dependencies, epistemic) = match target {
            RetrievalTarget::Cognition(reference) => {
                let value = self
                    .read_cognitive_source(ctx, ns, subject, reference)
                    .await?;
                let mut dependencies = value
                    .closure
                    .roots
                    .iter()
                    .map(|root| {
                        RetrievalTarget::Media(media::MediaReference::Original {
                            material_id: root.material_id.clone(),
                        })
                    })
                    .collect::<BTreeSet<_>>();
                for node in &value.closure.nodes {
                    let reference = match &node.source {
                        EvidenceSource::CognitiveRevision { reference } => {
                            Some(RetrievalTarget::Cognition(reference.clone()))
                        }
                        EvidenceSource::MemoryRevision {
                            memory_id,
                            revision,
                        } => Some(RetrievalTarget::Cognition(CognitiveRef::memory(
                            memory_id, *revision,
                        ))),
                        EvidenceSource::Material { material_id } => {
                            Some(RetrievalTarget::Media(media::MediaReference::Original {
                                material_id: material_id.clone(),
                            }))
                        }
                        EvidenceSource::Media { reference } => {
                            Some(RetrievalTarget::Media(reference.clone()))
                        }
                        EvidenceSource::Observation { .. } => None,
                    };
                    if let Some(reference) = reference {
                        dependencies.insert(reference);
                    }
                }
                let epistemic = self.context_epistemic(ns, subject, &value.object).await?;
                let text = match &value.object {
                    RetrievalRecord::Memory(memory) => {
                        let status = epistemic
                            .as_ref()
                            .and_then(|value| value.stance.as_ref())
                            .map(|stance| serde_json::to_value(stance.class))
                            .transpose()?
                            .and_then(|class| class.as_str().map(str::to_owned))
                            .unwrap_or_else(|| memory.epistemic_status.clone());
                        let event = match epistemic.as_ref().and_then(|value| value.grounding_kind)
                        {
                            Some(ObservationKind::Import) => "资料导入",
                            Some(ObservationKind::Contact) => "实际接触",
                            None => "未指定",
                        };
                        format!(
                            "{}\n{}\n认识状态：{}；来源事件：{}\n时间：{}",
                            memory.title,
                            memory.content,
                            status,
                            event,
                            serde_json::to_string(&memory.times)?
                        )
                    }
                    RetrievalRecord::Structured(object) => format!(
                        "{}\n时间：{}",
                        serde_json::to_string(&object.body)?,
                        serde_json::to_string(&object.times)?
                    ),
                    RetrievalRecord::Media(_) => {
                        return Err(Error::internal("cognition source reader returned media"));
                    }
                };
                (
                    text,
                    ContextAuthority::RecordedCognition,
                    json!({"object":value.object,"source":value.closure,"watermark":value.watermark}),
                    dependencies.into_iter().collect(),
                    epistemic,
                )
            }
            RetrievalTarget::Media(reference) => {
                let value = self.sources.resolve(ctx, ns, subject, reference).await?;
                let text = self.sources.text(&value, &CancellationToken::new()).await?;
                let lineage = self.sources.lineage(ctx, ns, subject, reference).await?;
                let mut dependencies = lineage
                    .materials
                    .iter()
                    .map(|original| {
                        RetrievalTarget::Media(media::MediaReference::Original {
                            material_id: original.material.id.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                dependencies.extend(
                    lineage
                        .source_regions
                        .iter()
                        .map(|region| RetrievalTarget::Media(region.reference())),
                );
                dependencies.extend(lineage.representations.iter().map(|representation| {
                    RetrievalTarget::Media(media::MediaReference::Representation {
                        reference: representation.reference(),
                    })
                }));
                (
                    text,
                    ContextAuthority::SourceMaterial,
                    json!({"source":value,"lineage":lineage}),
                    dependencies,
                    None,
                )
            }
        };
        Ok(ContextSegment {
            key: String::new(),
            text,
            sources: vec![target.clone()],
            required,
            dependencies,
            authority,
            provenance,
            epistemic,
        })
    }
    async fn context_epistemic(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
        object: &RetrievalRecord,
    ) -> Result<Option<ContextEpistemic>> {
        let (mut stance, role, grounding, qualifications, uncertain) = match object {
            RetrievalRecord::Memory(memory) => {
                let class = match memory.epistemic_status.as_str() {
                    "observed" => Some(EpistemicClass::Observed),
                    "reported" => Some(EpistemicClass::Reported),
                    "derived" => Some(EpistemicClass::Derived),
                    "inferred" => Some(EpistemicClass::Inferred),
                    "narrative" => Some(EpistemicClass::Narrative),
                    "simulated" => Some(EpistemicClass::Simulated),
                    _ => None,
                };
                (
                    class.map(|class| EpistemicStance {
                        class,
                        attribution: None,
                        confidence: None,
                    }),
                    None,
                    Some(memory.observation_id.clone()),
                    Vec::new(),
                    memory.epistemic_status == "uncertain",
                )
            }
            RetrievalRecord::Structured(value) => match &value.body {
                CognitiveContent::MemorySemantics(semantics) => (
                    Some(semantics.epistemic.clone()),
                    Some(semantics.cognitive_role),
                    semantics.grounding_observation.clone(),
                    semantics.qualifications.clone(),
                    false,
                ),
                CognitiveContent::Schema(schema) => (
                    Some(schema.epistemic.clone()),
                    None,
                    None,
                    Vec::new(),
                    false,
                ),
                CognitiveContent::Journal(journal) => (
                    Some(journal.perspective.clone()),
                    None,
                    None,
                    Vec::new(),
                    false,
                ),
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        let kind = if let Some(id) = &grounding {
            let event = self.database.observation(ns, id).await?;
            if event.subject_id != *subject {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "cognitive grounding belongs to another subject",
                ));
            }
            Some(event.kind)
        } else {
            None
        };
        if kind == Some(ObservationKind::Import) {
            let mut imported = stance.unwrap_or(EpistemicStance {
                class: EpistemicClass::Reported,
                attribution: None,
                confidence: None,
            });
            imported.class = EpistemicClass::Reported;
            stance = Some(imported);
        }
        Ok(Some(ContextEpistemic {
            stance,
            cognitive_role: role,
            grounding_kind: kind,
            source_event: grounding,
            qualifications,
            uncertain,
        }))
    }
}
