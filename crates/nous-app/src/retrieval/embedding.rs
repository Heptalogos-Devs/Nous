//! Shared admission and content reuse for every typed cognitive vector target.
use super::*;
use nous_foundation::models::ResolvedModel;
use nous_storage::{OperationKey, WriteTransaction};
use std::time::Duration;

pub(crate) fn provenance(record: &EmbeddingRecord) -> EmbeddingProvenance {
    EmbeddingProvenance {
        space_id: record.space_id.clone(),
        target: record.target.clone(),
        source_hash: record.source_hash.clone(),
        model_call_id: record.model_call_id.clone(),
        commit_seq: record.commit_seq,
    }
}

impl App {
    pub async fn sync_embeddings(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject: &SubjectId,
        limit: usize,
    ) -> Result<OperationReceipt> {
        if limit == 0 {
            return Err(Error::invalid("embedding snapshot limit must be positive"));
        }
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let config = self.embedding_configuration(&namespace, subject).await?;
        let mut policy = self.retrieval_policy(&namespace, subject).await?;
        policy.max_snapshot_entries = policy.max_snapshot_entries.min(limit);
        let request = QueryRequest {
            rerank: None,
            sort: None,
            expression: None,
            clock: None,
            history: false,
            filters: Default::default(),
            times: Vec::new(),
            source_read: None,
            mode: QueryMode::Lexical,
            subject_id: subject.clone(),
            input: QueryInput::default(),
            domains: vec![],
            reference: None,
            kinds: vec![],
            source: None,
            time: None,
            limit: policy.max_results,
            min_commit_seq: None,
            as_of_commit: None,
            include_inactive: true,
            include_deleted: false,
        };
        let mut view = self
            .query_view(ctx, &namespace, &request, &config.space()?.id, &policy)
            .await?;
        view.admit_routes(&config)?;
        let inputs = view.pending_inputs(&request)?;
        let scope = format!("namespace:{namespace}/subject:{subject}");
        let key = operation_key(
            ctx,
            "embedding.sync",
            &scope,
            operation_id,
            &json!({"subject_id":subject,"limit":limit,"space":config.space()?}),
        )?;
        self.admit_embedding_inputs(&namespace, subject, inputs, key, &config)
            .await
    }

    pub(crate) async fn queue_embedding_inputs(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        namespace: &NamespaceId,
        subject: &SubjectId,
        inputs: Vec<EmbeddingInputItem>,
        config: &ResolvedModel,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(namespace, subject)?;
        let mut targets = inputs
            .iter()
            .map(|input| Ok((input.source_key()?, input.source_hash())))
            .collect::<Result<Vec<_>>>()?;
        targets.sort();
        let key = operation_key(
            ctx,
            "embedding.inputs",
            &format!("namespace:{namespace}/subject:{subject}"),
            operation_id,
            &json!({"space":config.space()?,"inputs":targets}),
        )?;
        self.admit_embedding_inputs(namespace, subject, inputs, key, config)
            .await
    }

    pub(super) async fn embedding_configuration(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<ResolvedModel> {
        self.model
            .resolve(
                &ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                },
                super::embedding_model::ROLE,
            )
            .await
            .map_err(|error| error.to_domain_error())
    }

    async fn admit_embedding_inputs(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        inputs: Vec<EmbeddingInputItem>,
        key: OperationKey,
        config: &ResolvedModel,
    ) -> Result<OperationReceipt> {
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let space = config.space()?;
        tx.save_embedding_space(&space).await?;
        let mut unique = BTreeMap::<(String, String), EmbeddingInputItem>::new();
        for input in inputs {
            input.validate()?;
            let key = (input.source_key()?, input.source_hash());
            unique.entry(key).or_insert(input);
        }
        let claims = tx
            .embedding_claims(namespace, subject, &space.id)
            .await?
            .into_iter()
            .map(|claim| (claim.source_hash.clone(), claim))
            .collect::<BTreeMap<_, _>>();
        let mut fresh = BTreeMap::new();
        let mut jobs = BTreeSet::new();
        let mut reused = 0;
        let mut pending = 0;
        for (_, input) in unique {
            tx.subscribe_embedding(namespace, subject, &space.id, &input)
                .await?;
            if let Some(record) = tx
                .embedding_record(
                    namespace,
                    subject,
                    &space.id,
                    &input.target,
                    &input.source_hash(),
                )
                .await?
            {
                if record.source_hash != input.source_hash() {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "accepted vector differs from the source projection",
                    ));
                }
                continue;
            }
            tx.validate_embedding_input(namespace, subject, &input)
                .await?;
            if let Some(content) = tx
                .matching_embedding(namespace, subject, &space.id, &input.source_hash())
                .await?
            {
                let record = EmbeddingRecord {
                    namespace_id: content.namespace_id,
                    subject_id: content.subject_id,
                    space_id: content.space_id,
                    target: input.target.clone(),
                    source_hash: content.source_hash,
                    vector: content.vector,
                    model_call_id: content.model_call_id,
                    commit_seq: tx.commit_seq(),
                };
                tx.save_embedding_record(&record, &input).await?;
                reused += 1;
                continue;
            }
            pending += 1;
            if let Some(claim) = claims.get(&input.source_hash()) {
                jobs.insert(claim.job_id.clone());
            } else {
                fresh.entry(input.source_hash()).or_insert(input);
            }
        }
        let max_batch_size = config
            .routes
            .iter()
            .map(|route| route.parameters.max_batch_size)
            .min()
            .expect("resolved routes nonempty");
        let mut batch = Vec::new();
        for input in fresh.into_values() {
            let mut candidate = batch.clone();
            candidate.push(input.clone());
            let admitted = validate_batch(config, &candidate);
            if !batch.is_empty()
                && (candidate.len() > max_batch_size
                    || matches!(&admitted,Err(error) if matches!(error.code,ErrorCode::ResourceExhausted|ErrorCode::CapabilityUnavailable)))
            {
                jobs.insert(
                    self.queue_embedding_batch(
                        &mut tx,
                        namespace,
                        subject,
                        &space,
                        std::mem::take(&mut batch),
                    )
                    .await?,
                );
                validate_batch(config, std::slice::from_ref(&input))?;
                batch.push(input);
            } else {
                admitted?;
                batch = candidate;
            }
        }
        if !batch.is_empty() {
            jobs.insert(
                self.queue_embedding_batch(&mut tx, namespace, subject, &space, batch)
                    .await?,
            );
        }
        tx.commit(json!({"subject_id":subject,"space":space,"pending_inputs":pending,"reused_vectors":reused,"jobs":jobs})).await
    }

    async fn queue_embedding_batch(
        &self,
        tx: &mut WriteTransaction,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &EmbeddingSpace,
        inputs: Vec<EmbeddingInputItem>,
    ) -> Result<String> {
        let mut job = self.tasks.new_job(
            tx,
            "embedding",
            Some(namespace.clone()),
            Some(subject.clone()),
        );
        // Shared mathematical content owns no one subscriber's source-read selection.
        // Each accepted subscription is revalidated before representation and adoption.
        job.source_read.clear();
        self.tasks.submit_in(tx, &job).await?;
        tx.insert_embedding_run(&EmbeddingRun {
            job: job.clone(),
            space_id: space.id.clone(),
            content_hashes: inputs.iter().map(EmbeddingInputItem::source_hash).collect(),
        })
        .await?;
        Ok(job.id)
    }

    pub async fn get_embedding_run(&self, ctx: &CallContext, id: &str) -> Result<EmbeddingRun> {
        self.tasks.read(ctx, id).await?;
        self.database.embedding_run(id).await
    }

    pub(super) async fn wait_embeddings(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        jobs: &[String],
    ) -> Result<()> {
        let policy = self.retrieval_policy(namespace, subject).await?;
        let states = self
            .tasks
            .wait_all(
                ctx,
                jobs,
                Duration::from_millis(policy.embedding_wait_ms),
                &tokio_util::sync::CancellationToken::new(),
            )
            .await?;
        for job in states {
            if job.state != JobState::Succeeded {
                return Err(job.error.unwrap_or_else(|| {
                    Error::new(
                        ErrorCode::DependencyUnavailable,
                        format!("embedding task {} is {:?}", job.id, job.state),
                    )
                }));
            }
        }
        Ok(())
    }

    pub(super) async fn query_embedding(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        parts: Vec<EmbeddingPart>,
        read_requirements: &[SourceReadRef],
        config: &ResolvedModel,
    ) -> Result<EmbeddingRecord> {
        let space = config.space()?;
        let mut input = EmbeddingInputItem::query_parts(parts)?;
        input.read_requirements = read_requirements.to_vec();
        if let Some(record) = self
            .database
            .embedding_record(
                namespace,
                subject,
                &space.id,
                &input.target,
                &input.source_hash(),
            )
            .await?
        {
            return Ok(record);
        }
        let receipt = self
            .queue_embedding_inputs(
                ctx,
                &ulid::Ulid::generate().to_string(),
                namespace,
                subject,
                vec![input.clone()],
                config,
            )
            .await?;
        let jobs: Vec<String> = serde_json::from_value(receipt.result["jobs"].clone())?;
        self.wait_embeddings(ctx, namespace, subject, &jobs).await?;
        self.database
            .embedding_record(
                namespace,
                subject,
                &space.id,
                &input.target,
                &input.source_hash(),
            )
            .await?
            .ok_or_else(|| Error::internal("successful query vector is missing"))
    }
}

fn validate_batch(config: &ResolvedModel, inputs: &[EmbeddingInputItem]) -> Result<()> {
    let parts = inputs
        .iter()
        .map(|input| input.parts.clone())
        .collect::<Vec<_>>();
    let mut supported = false;
    for route in &config.routes {
        match route.validate_embedding_inputs(&parts) {
            Ok(()) => supported = true,
            Err(error) if error.code == ErrorCode::CapabilityUnavailable => {}
            Err(error) => return Err(error),
        }
    }
    if !supported {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "no embedding route supports these input parts",
        ));
    }
    Ok(())
}
