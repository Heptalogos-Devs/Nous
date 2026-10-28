use super::*;
use crate::bundle_format::Images;

impl Builder<'_> {
    pub(super) fn images(
        &mut self,
        images: &Images,
        digest: &str,
    ) -> Result<BTreeMap<String, String>> {
        let mut members = BTreeMap::new();
        let mut assets = BTreeMap::new();
        let mut occurrences = BTreeMap::new();
        let (namespace_id, subject_id) = self.scope();
        for asset in &images.assets {
            let id = MaterialId::new();
            let width = u32::try_from(asset.width)
                .map_err(|_| Error::invalid("image width exceeds authority range"))?;
            let height = u32::try_from(asset.height)
                .map_err(|_| Error::invalid("image height exceeds authority range"))?;
            let metadata = MediaMetadata::Image {
                width,
                height,
                frames: 1,
                duration_ms: None,
            };
            metadata.validate(&asset.mime_type)?;
            let material = Material {
                id: id.clone(),
                namespace_id: namespace_id.clone(),
                content_hash: asset.content_hash.clone(),
                media_type: asset.mime_type.clone(),
                metadata,
                byte_len: i64::try_from(asset.byte_size)
                    .map_err(|_| Error::invalid("image length exceeds authority range"))?,
                source: Some(format!("a-memorix:image-asset:{}", asset.asset_id)),
                event_time: None,
                recorded_at: self.package.created_at.clone(),
                commit_seq: 1,
            };
            self.package.materials.push(MediaOriginal {
                material,
                details: MaterialDetails {
                    source_type: Some("a_memorix_image".into()),
                    tags: vec![],
                    metadata: json!({"asset":asset}),
                    structured: None,
                },
                details_redacted: false,
            });
            assets.insert(asset.asset_id.clone(), id);
            members.insert(asset.content_hash.clone(), asset.member.clone());
        }
        for occurrence in &images.occurrences {
            let material_id = assets
                .get(&occurrence.asset_id)
                .cloned()
                .ok_or_else(|| Error::invalid("image occurrence asset not mapped"))?;
            let observation = Observation {
                id: ObservationId::new(),
                namespace_id: namespace_id.clone(),
                subject_id: subject_id.clone(),
                material_id: material_id.clone(),
                observed_at: None,
                origin: None,
                kind: ObservationKind::Import,
                recorded_at: self.package.created_at.clone(),
                commit_seq: 1,
            };
            let original_observations: Vec<_> = images
                .observations
                .iter()
                .filter(|value| value.occurrence_id == occurrence.occurrence_id)
                .collect();
            let original_links: Vec<_> = images
                .links
                .iter()
                .filter(|value| value.occurrence_id == occurrence.occurrence_id)
                .collect();
            let original = self
                .package
                .materials
                .iter_mut()
                .find(|value| value.material.id == material_id)
                .ok_or_else(|| Error::invalid("image occurrence original is absent"))?;
            let records = original
                .details
                .metadata
                .as_object_mut()
                .ok_or_else(|| Error::invalid("image source metadata must be an object"))?
                .entry("image_occurrences")
                .or_insert_with(|| json!([]));
            records.as_array_mut().ok_or_else(||Error::invalid("image occurrence records must be an array"))?
                .push(json!({"observation_id":observation.id,"record":occurrence,"observations":original_observations,"links":original_links}));
            occurrences.insert(
                occurrence.occurrence_id.clone(),
                (material_id, observation.id.clone()),
            );
            self.package.observations.push(observation);
        }
        for observation in &images.observations {
            let (material_id, observation_id) = occurrences
                .get(&observation.occurrence_id)
                .cloned()
                .ok_or_else(|| Error::invalid("image description occurrence not mapped"))?;
            if observation.text.trim().is_empty() {
                continue;
            }
            let reference = MediaReference::Original { material_id };
            let value = DerivedRepresentation {
                id: DerivedRepresentationId::new(),
                namespace_id: namespace_id.clone(),
                subject_id: subject_id.clone(),
                revision: 1,
                inputs: vec![DerivationInput {
                    ordinal: 0,
                    source: reference,
                    role: DerivationInputRole::Source,
                }],
                strategy: DerivationStrategy::DescriptionOnly,
                kind: RepresentationKind::ImageDescription,
                payload: RepresentationPayload::Text {
                    text: observation.text.clone(),
                },
                producer: DerivationProducer {
                    signature: archive::digest(observation)?,
                    implementation: "a-memorix.imported-description".into(),
                    model_call_id: None,
                    prompt_digest: None,
                    schema_digest: None,
                    config_digest: digest.into(),
                    preprocessing_digest: None,
                },
                access: ChannelAccess {
                    visual: ChannelMode::Representation,
                    audio: ChannelMode::Unavailable,
                    source_text: ChannelMode::Unavailable,
                },
                observation_id: Some(observation_id),
                source_event_kind: Some(ObservationKind::Import),
                timeline: None,
                formed_at: self.times().formation,
                recorded_at: self.package.created_at.clone(),
                commit_seq: 1,
                previous: None,
                quality: Default::default(),
            };
            self.package.representations.push(value);
        }
        for link in &images.links {
            let (material_id, _) = occurrences
                .get(&link.occurrence_id)
                .cloned()
                .ok_or_else(|| Error::invalid("image link occurrence not mapped"))?;
            let media = MediaReference::Original { material_id };
            let target = match link.target_type.as_str() {
                "paragraph" => self.memories.get(&link.target_id),
                "entity" => self.entities.get(&link.target_id),
                "relation" => self.relations.get(&link.target_id),
                "episode" => self.episodes.get(&link.target_id),
                _ => None,
            }
            .cloned()
            .ok_or_else(|| Error::invalid("image link target not mapped to typed knowledge"))?;
            if target.kind == CognitiveKind::Memory {
                let memory = self
                    .package
                    .memories
                    .iter()
                    .find(|m| m.id.as_str() == target.id)
                    .cloned()
                    .ok_or_else(|| Error::invalid("image paragraph target missing"))?;
                self.object(
                    CognitiveContent::MemorySemantics(MemorySemantics {
                        memory_id: memory.id,
                        memory_revision: memory.revision,
                        cognitive_role: CognitiveRole::Declarative,
                        formation_mode: FormationMode::Grounded,
                        epistemic: reported(),
                        grounding_observation: Some(memory.observation_id.clone()),
                        concepts: vec![],
                        qualifications: vec![format!("Imported image link: {}", link.link_kind)],
                    }),
                    vec![
                        Self::evidence(
                            EvidenceSource::Observation {
                                observation_id: memory.observation_id,
                            },
                            EvidencePolarity::Support,
                        ),
                        Self::evidence(
                            EvidenceSource::Media { reference: media },
                            EvidencePolarity::Context,
                        ),
                    ],
                )?;
            } else if let Some(object) = self
                .package
                .cognition
                .iter_mut()
                .find(|v| v.id.as_str() == target.id && v.revision == target.revision)
            {
                let source = EvidenceSource::Media {
                    reference: media.clone(),
                };
                if !object.evidence.iter().any(|v| v.source == source) {
                    object
                        .evidence
                        .push(Self::evidence(source, EvidencePolarity::Context));
                }
                if let CognitiveContent::Entity(entity) = &mut object.body {
                    entity.mentions.push(EntityMention {
                        target: SemanticTarget::from_reference(RetrievalTarget::Media(media)),
                        role: link.link_kind.clone(),
                    });
                }
                object.validate()?;
            }
        }
        Ok(members)
    }
}
