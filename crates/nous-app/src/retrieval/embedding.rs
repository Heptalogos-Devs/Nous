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
        if !(1..=10000).contains(&limit) {
            return Err(Error::invalid("embedding snapshot limit must be 1..10000"));
        }
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let config = self.embedding_configuration(&namespace, subject).await?;
        let snapshot = self
            .database
            .cognitive_retrieval_snapshot(&namespace, subject, &config.space()?.id, None, limit)
            .await?;
        let view = super::view::QueryView::from_snapshot(snapshot)?;
        let request = QueryRequest {
            mode: QueryMode::Lexical,
            subject_id: subject.clone(),
            text: String::new(),
            reference: None,
            kinds: vec![],
            source: None,
            time: None,
            limit: 100,
            min_commit_seq: None,
            as_of_commit: None,
            include_inactive: true,
            include_deleted: false,
        };
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
        let mut unique = BTreeMap::<String, EmbeddingInputItem>::new();
        for input in inputs {
            input.validate()?;
            let key = input.source_key()?;
            if let Some(previous) = unique.insert(key, input.clone())
                && previous != input
            {
                return Err(Error::invalid(
                    "embedding identity has inconsistent source text",
                ));
            }
        }
        let claims = tx
            .embedding_claims(namespace, subject, &space.id)
            .await?
            .into_iter()
            .map(|claim| Ok((claim.source_key()?, claim)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut fresh = Vec::new();
        let mut jobs = BTreeSet::new();
        let mut reused = 0;
        let mut pending = 0;
        for (key, input) in unique {
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
            if let Some(reference) = input.target.reference()
                && let Some(mut record) = tx
                    .embeddings_for_cognition(namespace, reference)
                    .await?
                    .into_iter()
                    .find(|record| {
                        record.subject_id == *subject
                            && record.space_id == space.id
                            && record.source_hash == input.source_hash()
                    })
            {
                record.target = input.target.clone();
                record.commit_seq = tx.commit_seq();
                tx.save_embedding_record(&record).await?;
                reused += 1;
                continue;
            }
            pending += 1;
            if let Some(claim) = claims.get(&key) {
                if claim.source_hash != input.source_hash() {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "pending vector target has changed source text",
                    ));
                }
                jobs.insert(claim.job_id.clone());
            } else {
                fresh.push(input);
            }
        }
        let max_input_bytes = config
            .routes
            .iter()
            .map(|route| route.parameters.max_input_bytes)
            .min()
            .expect("resolved routes nonempty");
        let max_batch_size = config
            .routes
            .iter()
            .map(|route| route.parameters.max_batch_size)
            .min()
            .expect("resolved routes nonempty");
        let mut batch = Vec::new();
        let mut bytes = 0;
        for input in fresh {
            if input.text.len() > max_input_bytes {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "embedding input exceeds max_input_bytes",
                ));
            }
            if !batch.is_empty()
                && (batch.len() >= max_batch_size || bytes + input.text.len() > max_input_bytes)
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
                bytes = 0;
            }
            bytes += input.text.len();
            batch.push(input);
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
        let job = self.tasks.new_job(
            "embedding.text",
            Some(namespace.clone()),
            Some(subject.clone()),
        );
        self.tasks.submit_in(tx, &job).await?;
        tx.insert_embedding_run(&EmbeddingRun {
            job: job.clone(),
            producer_signature: space.producer_signature.clone(),
            space_id: space.id.clone(),
            inputs,
        })
        .await?;
        Ok(job.id)
    }

    pub async fn get_embedding_run(&self, ctx: &CallContext, id: &str) -> Result<EmbeddingRun> {
        self.tasks.read(ctx, id).await?;
        self.database.embedding_run(id).await
    }

    pub(super) async fn wait_embeddings(&self, ctx: &CallContext, jobs: &[String]) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            let mut complete = true;
            for id in jobs {
                let job = self.tasks.read(ctx, id).await?;
                match job.state {
                    JobState::Succeeded => {}
                    JobState::Failed | JobState::Cancelled | JobState::Waiting => {
                        return Err(job.error.unwrap_or_else(|| {
                            Error::new(
                                ErrorCode::DependencyUnavailable,
                                format!("embedding task {id} is {:?}", job.state),
                            )
                        }));
                    }
                    _ => complete = false,
                }
            }
            if complete {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorCode::DependencyUnavailable,
                    format!("embedding tasks are still pending: {}", jobs.join(",")),
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(super) async fn query_embedding(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        text: &str,
        config: &ResolvedModel,
    ) -> Result<EmbeddingRecord> {
        let space = config.space()?;
        let input = EmbeddingInputItem::query(text)?;
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
        self.wait_embeddings(ctx, &jobs).await?;
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
