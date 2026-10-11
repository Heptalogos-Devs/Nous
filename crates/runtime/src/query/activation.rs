//! One query-local semantic activation contract shared by every retrieval profile.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use nous_configuration::*;
use nous_core::*;
use serde::{Deserialize, Serialize};
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ConceptEnrichment {
    #[default]
    Off,
    Existing,
    Model,
}
pub const CONCEPT_ENRICHMENT: ConfigKey<ConceptEnrichment> =
    ConfigKey::new("retrieval.query.concept_enrichment");
pub const MAX_ACTIVATED_TAGS: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.concept_activation.max_activated_tags");
pub const CONCEPT_MINIMUM_SIMILARITY: ConfigKey<f64> =
    ConfigKey::new("retrieval.query.concept_activation.minimum_similarity");
pub const CONCEPT_MODEL_CATALOG_LIMIT: ConfigKey<usize> =
    ConfigKey::new("retrieval.query.concept_activation.model_catalog_limit");

#[derive(Debug, Clone)]
pub struct ConceptActivationPolicy {
    pub max_activated_tags: usize,
    pub minimum_similarity: f64,
    pub model_catalog_limit: usize,
}
impl ConceptActivationPolicy {
    pub fn from_snapshot(snapshot: &ConfigSnapshot) -> Result<Self> {
        Ok(Self {
            max_activated_tags: snapshot.get(MAX_ACTIVATED_TAGS)?,
            minimum_similarity: snapshot.get(CONCEPT_MINIMUM_SIMILARITY)?,
            model_catalog_limit: snapshot.get(CONCEPT_MODEL_CATALOG_LIMIT)?,
        })
    }
}

pub(super) fn register(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        CONCEPT_ENRICHMENT,
        "runtime",
        "Query-local concept enrichment; never writes Tag Authority.",
        ConceptEnrichment::Off,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::QueryPolicy,
        |_: &ConceptEnrichment| Ok(()),
    )?;
    let reference = ReferenceProfile::parse(include_str!(
        "../../../../config/reference/query-concept-activation.json"
    ))?;
    for (key, description) in [
        (
            MAX_ACTIVATED_TAGS,
            "Maximum inferred concepts retained by one query.",
        ),
        (
            CONCEPT_MODEL_CATALOG_LIMIT,
            "Maximum frozen concept catalog entries supplied to the query model.",
        ),
    ] {
        registry.register(
            key,
            "runtime",
            description,
            reference.get(key)?,
            ConfigExposure::Advanced,
            ConfigScopePolicy::SubjectOverrideAllowed,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::QueryPolicy,
            |value| {
                if *value > 0 {
                    Ok(())
                } else {
                    Err(Error::Invalid("concept budget must be positive".into()))
                }
            },
        )?;
        registry.describe(key.path(), |descriptor| {
            descriptor.json_schema["minimum"] = serde_json::json!(1);
            descriptor.unit = Some("items".into());
        })?;
    }
    registry.register(
        CONCEPT_MINIMUM_SIMILARITY,
        "runtime",
        "Minimum cosine similarity for existing concept activation.",
        reference.get(CONCEPT_MINIMUM_SIMILARITY)?,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::QueryPolicy,
        |value| {
            if value.is_finite() && (-1.0..=1.0).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "concept similarity must be finite and within -1..1".into(),
                ))
            }
        },
    )?;
    registry.bounds(CONCEPT_MINIMUM_SIMILARITY, -1.0, 1.0, Some("cosine"))?;
    reference.describe(registry)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationSource {
    ExplicitQuery,
    ExistingSemanticMatch,
    ModelInferred,
    EntityContext,
    WorkContext,
    RuntimeContext,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagActivation {
    pub tag: TagId,
    pub strength: f64,
    pub source: ActivationSource,
    pub origin: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NovelConceptHypothesis {
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationSeed {
    pub reference: CognitiveRef,
    pub origin: String,
    pub strength: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuerySemanticEmbedding {
    pub vector: Vec<f32>,
    pub space: EmbeddingSpaceSignature,
    pub producer: ProducerSignature,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryActivation {
    pub concept_catalog: Vec<super::QueryConceptCandidate>,
    pub frozen: bool,
    pub semantic_text_digest: String,
    pub exact_refs: Vec<CognitiveRef>,
    pub entity_refs: Vec<EntityRef>,
    pub explicit_tags: Vec<TagActivation>,
    pub inferred_tags: Vec<TagActivation>,
    pub novel_concepts: Vec<NovelConceptHypothesis>,
    pub schema_refs: Vec<CognitiveRef>,
    pub runtime_refs: Vec<CognitiveRef>,
    pub seeds: Vec<ActivationSeed>,
    pub exploration: ExplorationIntent,
    pub temporal_frame: TemporalFrame,
    pub concept_generation: Option<ServingGenerationId>,
    pub query_embedding_digest: Option<String>,
    #[serde(skip)]
    pub embedding: Option<QuerySemanticEmbedding>,
    pub degradation: Vec<Degradation>,
    pub model_calls: usize,
    pub model_completed: bool,
}
impl QueryActivation {
    pub fn prepared(
        query: &CognitiveQuery,
        digest: String,
        exact_refs: Vec<CognitiveRef>,
        runtime_refs: Vec<CognitiveRef>,
        structural_seeds: &[(CognitiveRef, String)],
    ) -> Self {
        let mut seeds = exact_refs
            .iter()
            .cloned()
            .map(|reference| ActivationSeed {
                reference,
                origin: "exact_target".into(),
                strength: 1.0,
            })
            .chain(
                runtime_refs
                    .iter()
                    .cloned()
                    .map(|reference| ActivationSeed {
                        reference,
                        origin: "runtime_situation".into(),
                        strength: 1.0,
                    }),
            )
            .collect::<Vec<_>>();
        seeds.extend(
            structural_seeds
                .iter()
                .cloned()
                .map(|(reference, origin)| ActivationSeed {
                    reference,
                    origin,
                    strength: 1.0,
                }),
        );
        seeds.extend(
            query
                .scopes()
                .into_iter()
                .flat_map(|scope| &scope.targets)
                .filter_map(|target| {
                    if let QueryTarget::EntityNeighborhood { entity_ref } = target {
                        Some(ActivationSeed {
                            reference: CognitiveRef::Entity(entity_ref.clone()),
                            origin: "entity_cue".into(),
                            strength: 1.0,
                        })
                    } else {
                        None
                    }
                }),
        );
        for cue in query.scopes().into_iter().flat_map(|scope| &scope.cues) {
            let value = match cue {
                Cue::Tag(tag) => Some((CognitiveRef::Tag(tag.tag), "tag_cue")),
                Cue::Entity(entity) => Some((
                    CognitiveRef::Entity(entity.entity_ref.clone()),
                    "entity_cue",
                )),
                _ => None,
            };
            if let Some((reference, origin)) = value {
                seeds.push(ActivationSeed {
                    reference,
                    origin: origin.into(),
                    strength: 1.0,
                });
            }
        }
        seeds.sort_by_key(|s| (s.reference.to_string(), s.origin.clone()));
        seeds.dedup_by(|a, b| a.reference == b.reference && a.origin == b.origin);
        let explicit_tags = explicit_tag_activations(&seeds);
        let entity_refs = seeds
            .iter()
            .filter_map(|s| {
                if let CognitiveRef::Entity(e) = &s.reference {
                    Some(e.clone())
                } else {
                    None
                }
            })
            .collect();
        let schema_refs = seeds
            .iter()
            .filter(|s| {
                matches!(
                    s.reference,
                    CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_)
                )
            })
            .map(|s| s.reference.clone())
            .collect();
        Self {
            concept_catalog: vec![],
            frozen: false,
            semantic_text_digest: digest,
            exact_refs,
            entity_refs,
            explicit_tags,
            inferred_tags: vec![],
            novel_concepts: vec![],
            schema_refs,
            runtime_refs,
            seeds,
            exploration: query.exploration,
            temporal_frame: query.temporal_frame.clone(),
            concept_generation: None,
            query_embedding_digest: None,
            embedding: None,
            degradation: vec![],
            model_calls: 0,
            model_completed: false,
        }
    }
    pub fn tags(&self) -> impl Iterator<Item = &TagActivation> {
        self.explicit_tags.iter().chain(&self.inferred_tags)
    }
    pub fn scoped(
        &self,
        query: &CognitiveQuery,
        exact_refs: Vec<CognitiveRef>,
        structural_seeds: &[(CognitiveRef, String)],
    ) -> Self {
        let mut scoped = Self::prepared(
            query,
            self.semantic_text_digest.clone(),
            exact_refs,
            self.runtime_refs.clone(),
            structural_seeds,
        );
        scoped.concept_catalog = self.concept_catalog.clone();
        scoped.frozen = self.frozen;
        scoped.inferred_tags = self.inferred_tags.clone();
        scoped.novel_concepts = self.novel_concepts.clone();
        scoped.embedding = self.embedding.clone();
        scoped.query_embedding_digest = self.query_embedding_digest.clone();
        scoped.concept_generation = self.concept_generation;
        scoped.degradation = self.degradation.clone();
        scoped.model_calls = self.model_calls;
        scoped.model_completed = self.model_completed;
        scoped
            .seeds
            .extend(scoped.inferred_tags.iter().map(|tag| ActivationSeed {
                reference: CognitiveRef::Tag(tag.tag),
                origin: tag.origin.clone(),
                strength: tag.strength,
            }));
        scoped
    }
}

fn explicit_tag_activations(seeds: &[ActivationSeed]) -> Vec<TagActivation> {
    seeds
        .iter()
        .filter_map(|seed| {
            if let CognitiveRef::Tag(tag) = seed.reference {
                Some(TagActivation {
                    tag,
                    strength: 1.0,
                    source: if seed.origin == "work_context" {
                        ActivationSource::WorkContext
                    } else if seed.origin == "runtime_situation" {
                        ActivationSource::RuntimeContext
                    } else {
                        ActivationSource::ExplicitQuery
                    },
                    origin: seed.origin.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}
