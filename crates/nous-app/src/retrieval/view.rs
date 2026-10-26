use super::{config::RetrievalPolicy, *};
use nous_domain::media::{MediaMetadata, RepresentationPayload};
use nous_foundation::models::ResolvedModel;
use nous_storage::CognitiveRetrievalSnapshot;
use tokio_util::sync::CancellationToken;
mod entry;
pub(crate) use entry::Entry;

pub(crate) struct QueryView {
    pub watermark: i64,
    pub entries: Vec<Entry>,
    pub tags: BTreeMap<String, EmbeddingRecord>,
    pub issues: Vec<ProjectionIssue>,
}
impl QueryView {
    fn new(watermark: i64) -> Self {
        Self {
            watermark,
            entries: Vec::new(),
            tags: BTreeMap::new(),
            issues: Vec::new(),
        }
    }
    fn add_cognition(&mut self, snapshot: CognitiveRetrievalSnapshot) -> Result<()> {
        if !snapshot.complete {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "cognition exceeds the configured snapshot limit",
            ));
        }
        for entry in snapshot.memories {
            let input = EmbeddingInputItem::for_memory(&entry.memory)?;
            self.entries.push(Entry {
                recorded: entry.memory.times.recorded.clone(),
                object: RetrievalRecord::Memory(entry.memory),
                lexical_text: input.text_content().map(str::to_owned),
                input: Some(input),
                embedding: entry.embedding,
                roots: entry
                    .source
                    .roots
                    .into_iter()
                    .map(|root| root.material_id)
                    .collect(),
                materials: entry.materials,
                sources_complete: entry.source.complete,
                media_source: None,
                observations: Vec::new(),
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
            self.entries.push(Entry {
                recorded: entry.object.times.recorded.clone(),
                object: RetrievalRecord::Structured(entry.object),
                lexical_text: input
                    .as_ref()
                    .and_then(EmbeddingInputItem::text_content)
                    .map(str::to_owned),
                input,
                embedding: entry.embedding,
                roots: entry
                    .source
                    .roots
                    .into_iter()
                    .map(|root| root.material_id)
                    .collect(),
                materials: entry.materials,
                sources_complete: entry.source.complete,
                media_source: None,
                observations: Vec::new(),
            });
        }
        for record in snapshot.tag_embeddings {
            self.tags.insert(record.source_key()?, record);
        }
        Ok(())
    }
    pub fn admit_routes(&mut self, model: &ResolvedModel) -> Result<()> {
        for entry in &mut self.entries {
            if entry.accepted_embedding().is_some() {
                continue;
            }
            let Some(input) = entry.input.as_ref() else {
                continue;
            };
            let mut rejection = None;
            let mut accepted = false;
            for route in &model.routes {
                match route.validate_embedding_inputs(std::slice::from_ref(&input.parts)) {
                    Ok(()) => {
                        accepted = true;
                        break;
                    }
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::CapabilityUnavailable | ErrorCode::ResourceExhausted
                        ) =>
                    {
                        rejection = Some(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            if !accepted {
                self.issues.push(ProjectionIssue {
                    target: entry.reference(),
                    error: rejection.unwrap_or_else(|| {
                        Error::new(
                            ErrorCode::CapabilityUnavailable,
                            "no embedding route is available for this projection",
                        )
                    }),
                });
                entry.input = None;
            }
        }
        Ok(())
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

    pub async fn refresh_embeddings(
        &mut self,
        database: &nous_storage::Database,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
    ) -> Result<()> {
        let mut read = database.read_view().await?;
        let mut tags = BTreeMap::new();
        for entry in &mut self.entries {
            if let Some(input) = &entry.input {
                entry.embedding = read
                    .embedding_record(
                        namespace,
                        subject,
                        space,
                        &input.target,
                        &input.source_hash(),
                    )
                    .await?;
            }
            for (_, tag) in entry.tags()? {
                tags.insert(tag.source_key()?, tag);
            }
        }
        for (key, input) in tags {
            if let Some(record) = read
                .embedding_record(
                    namespace,
                    subject,
                    space,
                    &input.target,
                    &input.source_hash(),
                )
                .await?
            {
                self.tags.insert(key, record);
            }
        }
        read.finish().await
    }
}
impl App {
    pub(crate) async fn query_view(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        request: &QueryRequest,
        space: &str,
        policy: &RetrievalPolicy,
    ) -> Result<QueryView> {
        let cancel = CancellationToken::new();
        let _protection = self.objects.protect_references(&cancel).await?;
        let mut read = self.database.read_view().await?;
        read.subject(namespace, &request.subject_id).await?;
        let source_read = read
            .source_read_view(ctx, namespace, &request.subject_id)
            .await?;
        let mut output = QueryView::new(read.watermark);
        let mut media_entries = Vec::new();
        let exact = request.reference.as_ref();
        if request.wants_domain(QueryDomain::Cognition)
            && exact.is_none_or(|value| value.cognition().is_some())
        {
            let snapshot = if let Some(reference) = exact.and_then(RetrievalTarget::cognition) {
                read.cognitive_reference_snapshot(
                    namespace,
                    &request.subject_id,
                    space,
                    reference,
                    request.as_of_commit,
                    Some(&source_read),
                )
                .await?
            } else {
                read.cognitive_retrieval_snapshot(
                    namespace,
                    &request.subject_id,
                    space,
                    request.as_of_commit,
                    policy.max_snapshot_entries,
                    Some(&source_read),
                )
                .await?
            };
            output.add_cognition(snapshot)?;
        }
        if request.wants_domain(QueryDomain::Media)
            && request.kinds.is_empty()
            && exact.is_none_or(|value| value.media().is_some())
        {
            let snapshot = if let Some(reference) = exact.and_then(RetrievalTarget::media) {
                read.media_reference_snapshot(
                    namespace,
                    &request.subject_id,
                    space,
                    reference,
                    request.as_of_commit,
                    Some(&source_read),
                )
                .await?
            } else {
                read.media_retrieval_snapshot(
                    namespace,
                    &request.subject_id,
                    space,
                    request.as_of_commit,
                    policy.max_snapshot_entries,
                    Some(&source_read),
                )
                .await?
            };
            if !snapshot.complete
                || output.entries.len().saturating_add(snapshot.entries.len())
                    > policy.max_snapshot_entries
            {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "media and cognition exceed the configured snapshot limit",
                ));
            }
            for entry in snapshot.entries {
                let object = MediaRecord::from_lineage(&entry.source)?;
                let reference = entry.source.source.clone();
                let source = self
                    .sources
                    .resolve_in(
                        ctx,
                        &mut read,
                        namespace,
                        &request.subject_id,
                        &reference,
                        None,
                    )
                    .await?;
                let recorded = OperationTime::parse(object.recorded_at())?.point();
                let candidate = Entry {
                    object: RetrievalRecord::Media(object),
                    lexical_text: None,
                    input: None,
                    embedding: None,
                    roots: entry
                        .source
                        .materials
                        .iter()
                        .map(|root| root.material.id.clone())
                        .collect(),
                    materials: entry.source.materials.clone(),
                    sources_complete: true,
                    media_source: Some(entry.source),
                    observations: entry.observations,
                    recorded,
                };
                media_entries.push((candidate, source));
            }
        }
        read.finish().await?;
        // Frozen semantic facts remain in memory; mechanical I/O and writes hold no SQL read transaction.
        for (mut entry, source) in media_entries {
            if !entry.eligible(request) {
                output.entries.push(entry);
                continue;
            }
            let target = entry.reference();
            let is_text = matches!(
                source.payload,
                RepresentationPayload::Text { .. }
                    | RepresentationPayload::Interpretation { .. }
                    | RepresentationPayload::Artifact {
                        metadata: MediaMetadata::Text,
                        ..
                    }
            );
            entry.lexical_text = if is_text {
                match self.sources.text(&source, &cancel).await {
                    Ok(text) => Some(text),
                    Err(error) if error.code == ErrorCode::ResourceExhausted => {
                        output.issues.push(ProjectionIssue {
                            target: target.clone(),
                            error,
                        });
                        None
                    }
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            entry.input = if space.is_empty() || (is_text && entry.lexical_text.is_none()) {
                None
            } else {
                match self
                    .embedding_media_parts(
                        ctx,
                        namespace,
                        &request.subject_id,
                        &source.reference,
                        &cancel,
                    )
                    .await
                {
                    Ok(parts) => Some(EmbeddingInputItem::new(
                        EmbeddingTarget::Item(target.clone()),
                        parts,
                    )?),
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::CapabilityUnavailable | ErrorCode::ResourceExhausted
                        ) =>
                    {
                        output.issues.push(ProjectionIssue {
                            target: target.clone(),
                            error,
                        });
                        None
                    }
                    Err(error) => return Err(error),
                }
            };
            entry.embedding = if let Some(input) = &entry.input {
                self.database
                    .embedding_record(
                        namespace,
                        &request.subject_id,
                        space,
                        &input.target,
                        &input.source_hash(),
                    )
                    .await?
            } else {
                None
            };
            output.entries.push(entry);
        }
        output.entries.sort_by_key(Entry::reference);
        Ok(output)
    }
}
