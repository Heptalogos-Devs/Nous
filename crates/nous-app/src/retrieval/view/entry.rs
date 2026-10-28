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
                request.wants_domain(QueryDomain::Media)
                    && (request.kinds.is_empty() || request.domains.contains(&QueryDomain::Media))
            }
        };
        kind_matches
            && request.filters.matches_record(&self.object)
            && self.matches_times(request)
            && request
                .reference
                .as_ref()
                .is_none_or(|requested| requested == &reference)
            && (request.include_deleted || !self.object.deleted())
            && (request.include_inactive
                || request.reference.is_some()
                || (request.include_deleted && self.object.deleted())
                || self.object.is_active())
            && request.source.as_ref().is_none_or(|source| {
                self.materials.iter().any(|root| {
                    material_source_name(&root.material, &root.details) == Some(source.as_str())
                })
            })
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
                let mut input = EmbeddingInputItem::tag(label)?;
                input
                    .read_requirements
                    .push(SourceReadRef::target(&self.reference()));
                let source = format!("material:{}", root.material.id);
                if seen.insert((source.clone(), input.source_key()?)) {
                    tags.push((source, input));
                }
            }
        }
        Ok(tags)
    }
}
