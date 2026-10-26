use super::*;
use nous_domain::media::{MediaOriginal, MediaSourceLineage};

pub(crate) struct Entry {
    pub object: RetrievalRecord,
    pub input: Option<EmbeddingInputItem>,
    pub lexical_text: Option<String>,
    pub embedding: Option<EmbeddingRecord>,
    pub roots: Vec<MaterialId>,
    pub materials: Vec<MediaOriginal>,
    pub sources_complete: bool,
    pub media_source: Option<MediaSourceLineage>,
    pub observations: Vec<Observation>,
    pub recorded: TimePoint,
}
impl Entry {
    pub fn reference(&self) -> RetrievalTarget {
        self.object.reference()
    }

    pub fn eligible(&self, request: &QueryRequest) -> bool {
        let reference = self.reference();
        let kind_matches = match &reference {
            RetrievalTarget::Cognition(value) => {
                request.wants_domain(QueryDomain::Cognition)
                    && (request.kinds.is_empty() || request.kinds.contains(&value.kind))
            }
            RetrievalTarget::Media(_) => {
                request.wants_domain(QueryDomain::Media) && request.kinds.is_empty()
            }
        };
        kind_matches
            && request
                .reference
                .as_ref()
                .is_none_or(|requested| requested == &reference)
            && (request.include_deleted || !self.object.deleted())
            && (request.include_inactive
                || request.reference.is_some()
                || (request.include_deleted && self.object.deleted())
                || self.object.is_active())
            && request
                .time
                .as_ref()
                .is_none_or(|filter| self.matches_time(filter))
            && request.source.as_ref().is_none_or(|source| {
                self.materials
                    .iter()
                    .any(|root| root.material.source.as_ref() == Some(source))
            })
    }
    fn matches_time(&self, filter: &TimeFilter) -> bool {
        if let Some(times) = self.object.cognitive_times() {
            return filter.matches(times);
        }
        match filter.axis {
            TimeAxis::Event => self.materials.iter().any(|root| {
                root.material
                    .event_time
                    .as_ref()
                    .is_some_and(|time| filter.range.contains(time))
            }),
            TimeAxis::Observation => self.observations.iter().any(|event| {
                event
                    .observed_at
                    .as_ref()
                    .is_some_and(|time| filter.range.contains(time))
            }),
            TimeAxis::Formation => {
                let reference = match &self.object {
                    RetrievalRecord::Media(MediaRecord::Representation(value)) => {
                        Some(value.reference())
                    }
                    RetrievalRecord::Media(MediaRecord::DerivedRegion(value)) => {
                        Some(value.representation.clone())
                    }
                    _ => None,
                };
                self.media_source
                    .as_ref()
                    .zip(reference)
                    .is_some_and(|(source, reference)| {
                        source.representations.iter().any(|value| {
                            value.reference() == reference
                                && filter.range.contains(&value.formed_at)
                        })
                    })
            }
            TimeAxis::Recorded => filter.range.contains(&self.recorded),
            TimeAxis::Validity => false,
        }
    }
    pub fn can_embed(&self) -> bool {
        !self.object.deleted() && self.sources_complete && self.input.is_some()
    }
    pub fn accepted_embedding(&self) -> Option<&EmbeddingRecord> {
        self.embedding.as_ref().filter(|record| {
            self.input.as_ref().is_some_and(|input| {
                record.target == input.target && record.source_hash == input.source_hash()
            })
        })
    }
    pub fn tags(&self) -> Result<Vec<(String, EmbeddingInputItem)>> {
        let mut tags = Vec::new();
        let mut seen = BTreeSet::new();
        for root in &self.materials {
            for label in &root.details.tags {
                if label.trim().is_empty() {
                    continue;
                }
                let input = EmbeddingInputItem::tag(label)?;
                let source = format!("material:{}", root.material.id);
                if seen.insert((source.clone(), input.source_key()?)) {
                    tags.push((source, input));
                }
            }
        }
        Ok(tags)
    }
}
