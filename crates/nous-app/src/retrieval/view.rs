//! One typed projection shared by lexical, vector, graph and exact-reference reads.
use super::*;
use nous_storage::{CognitiveRetrievalSnapshot, RootMaterial};

pub(super) struct Entry {
    pub object: CognitiveRecord,
    pub input: Option<EmbeddingInputItem>,
    pub embedding: Option<EmbeddingRecord>,
    pub roots: Vec<MaterialId>,
    pub materials: Vec<RootMaterial>,
    pub sources_complete: bool,
}
impl Entry {
    pub fn reference(&self) -> CognitiveRef {
        self.object.reference()
    }
    pub fn eligible(&self, request: &QueryRequest) -> bool {
        let reference = self.reference();
        (request.kinds.is_empty() || request.kinds.contains(&reference.kind))
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
                .is_none_or(|filter| filter.matches(self.object.times()))
            && request.source.as_ref().is_none_or(|source| {
                self.materials
                    .iter()
                    .any(|root| root.material.source.as_ref() == Some(source))
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

pub(super) struct QueryView {
    pub watermark: i64,
    pub entries: Vec<Entry>,
    pub tags: BTreeMap<String, EmbeddingRecord>,
}
impl QueryView {
    pub(super) fn from_snapshot(snapshot: CognitiveRetrievalSnapshot) -> Result<Self> {
        if !snapshot.complete {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "cognitive snapshot exceeds 10000 objects",
            ));
        }
        let mut entries = Vec::with_capacity(snapshot.memories.len() + snapshot.structured.len());
        for entry in snapshot.memories {
            let input = EmbeddingInputItem::for_memory(&entry.memory)?;
            entries.push(Entry {
                roots: entry
                    .source
                    .roots
                    .into_iter()
                    .map(|root| root.material_id)
                    .collect(),
                materials: entry.materials,
                sources_complete: entry.source.complete,
                object: CognitiveRecord::Memory(entry.memory),
                input: Some(input),
                embedding: entry.embedding,
            });
        }
        for entry in snapshot.structured {
            let input = if matches!(entry.object.body, CognitiveContent::MemorySemantics(_))
                && entry.base_memory.is_none()
            {
                None
            } else {
                Some(EmbeddingInputItem::for_cognition(
                    &entry.object,
                    entry.base_memory.as_ref(),
                )?)
            };
            entries.push(Entry {
                roots: entry
                    .source
                    .roots
                    .into_iter()
                    .map(|root| root.material_id)
                    .collect(),
                materials: entry.materials,
                sources_complete: entry.source.complete,
                object: CognitiveRecord::Structured(entry.object),
                input,
                embedding: entry.embedding,
            });
        }
        entries.sort_by_key(Entry::reference);
        Ok(Self {
            watermark: snapshot.watermark,
            entries,
            tags: snapshot
                .tag_embeddings
                .into_iter()
                .map(|record| Ok((record.source_key()?, record)))
                .collect::<Result<_>>()?,
        })
    }
    pub fn selected<'a>(&'a self, request: &'a QueryRequest) -> impl Iterator<Item = &'a Entry> {
        self.entries.iter().filter(|entry| entry.eligible(request))
    }
    pub fn pending_inputs(&self, request: &QueryRequest) -> Result<Vec<EmbeddingInputItem>> {
        let mut pending = BTreeMap::new();
        for entry in self.selected(request).filter(|entry| entry.can_embed()) {
            if entry.accepted_embedding().is_none() {
                let input = entry.input.as_ref().expect("embeddable input");
                pending.insert(input.source_key()?, input.clone());
            }
            for (_, input) in entry.tags()? {
                let key = input.source_key()?;
                if !self.tags.contains_key(&key) {
                    pending.insert(key, input);
                }
            }
        }
        Ok(pending.into_values().collect())
    }
}
impl App {
    pub(super) async fn query_view(
        &self,
        namespace: &NamespaceId,
        request: &QueryRequest,
        space: &str,
    ) -> Result<QueryView> {
        let mut view = self.database.read_view().await?;
        let snapshot = if let Some(reference) = &request.reference {
            view.cognitive_reference_snapshot(
                namespace,
                &request.subject_id,
                space,
                reference,
                request.as_of_commit,
            )
            .await?
        } else {
            view.cognitive_retrieval_snapshot(
                namespace,
                &request.subject_id,
                space,
                request.as_of_commit,
                10000,
            )
            .await?
        };
        view.finish().await?;
        QueryView::from_snapshot(snapshot)
    }
}
