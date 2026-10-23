//! Durable text representations. Queue and ModelCall mechanics are shared with formation.
use super::*;
use crate::calls::{CallRequest, budget_error, complete_redacted_call, settle_call};
use model::RetryClass;
use nous_storage::{JobEnvelope, WriteTransaction};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::Duration,
};

pub(crate) fn text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
pub(crate) fn memory_input(memory: &Memory) -> EmbeddingInputItem {
    let text = format!("{}\n{}", memory.title, memory.content);
    EmbeddingInputItem {
        kind: EmbeddingKind::Memory,
        source_key: format!("memory:{}@{}", memory.id, memory.revision),
        source_hash: text_hash(&text),
        text,
        memory_id: Some(memory.id.clone()),
        memory_revision: Some(memory.revision),
    }
}
pub(crate) fn text_input(kind: EmbeddingKind, text: &str) -> EmbeddingInputItem {
    let text = text.trim().to_owned();
    let source_hash = text_hash(&text);
    let prefix = if kind == EmbeddingKind::Tag {
        "tag"
    } else {
        "query"
    };
    EmbeddingInputItem {
        kind,
        source_key: format!("{prefix}:{source_hash}"),
        source_hash,
        text,
        memory_id: None,
        memory_revision: None,
    }
}
pub(crate) fn provenance(record: &EmbeddingRecord) -> EmbeddingProvenance {
    EmbeddingProvenance {
        space_id: record.space_id.clone(),
        source_key: record.source_key.clone(),
        source_hash: record.source_hash.clone(),
        memory_revision: record.memory_revision,
        model_call_id: record.model_call_id.clone(),
        commit_seq: record.commit_seq,
    }
}

impl App {
    pub(crate) async fn schedule_embedding_catchup(&self) -> Result<()> {
        if self.model.embedding_config().is_none() {
            return Ok(());
        }
        let admin = CallContext::admin();
        for namespace in self.list_namespaces(&admin).await? {
            for subject in self.list_subjects(&admin, &namespace.id).await? {
                self.sync_embeddings(
                    &admin,
                    &ulid::Ulid::generate().to_string(),
                    &subject.id,
                    5000,
                )
                .await?;
            }
        }
        Ok(())
    }
    pub async fn sync_embeddings(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject: &SubjectId,
        limit: usize,
    ) -> Result<OperationReceipt> {
        self.sync_embeddings_filtered(ctx, operation_id, subject, limit, None)
            .await
    }
    pub(crate) async fn sync_embeddings_filtered(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject: &SubjectId,
        limit: usize,
        filter: Option<&QueryRequest>,
    ) -> Result<OperationReceipt> {
        if !(1..=5000).contains(&limit) {
            return Err(Error::invalid("embedding snapshot limit must be 1..5000"));
        }
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let config = self
            .model
            .embedding_config()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "text embedding resource is not configured",
                )
            })?
            .clone();
        let space = config.space();
        let mut snapshot = self
            .database
            .retrieval_snapshot(&namespace, subject, &space.id, limit)
            .await?;
        if !snapshot.complete {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "subject exceeds the embedding snapshot limit",
            ));
        }
        if let Some(filter) = filter {
            snapshot
                .entries
                .retain(|entry| crate::semantic::eligible(entry, filter));
        }
        let scope = format!("namespace:{namespace}/subject:{subject}");
        let key = operation_key(
            ctx,
            "embedding.sync",
            &scope,
            operation_id,
            &json!({"subject_id":subject,"limit":limit,"space_id":space.id}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        tx.save_embedding_space(&space).await?;
        let mut pending = Vec::new();
        let mut tags = BTreeMap::new();
        let mut reused = 0;
        for entry in snapshot
            .entries
            .iter()
            .filter(|entry| !entry.memory.deleted)
        {
            let input = memory_input(&entry.memory);
            if entry
                .embedding
                .as_ref()
                .is_none_or(|record| record.source_hash != input.source_hash)
            {
                let previous = self
                    .database
                    .embeddings_for_memory(&namespace, &entry.memory.id)
                    .await?;
                if let Some(previous) = previous.into_iter().find(|record| {
                    record.space_id == space.id && record.source_hash == input.source_hash
                }) {
                    let mut record = previous;
                    if record.source_key != input.source_key {
                        record.source_key = input.source_key;
                        record.memory_revision = input.memory_revision;
                        record.commit_seq = tx.commit_seq();
                        tx.save_embedding_record(&record).await?;
                    }
                    reused += 1;
                } else {
                    pending.push(input);
                }
            }
            for label in &entry.details.tags {
                let input = text_input(EmbeddingKind::Tag, label);
                if !input.text.is_empty() {
                    tags.insert(input.source_key.clone(), input);
                }
            }
        }
        let stored_tags: BTreeSet<_> = snapshot
            .tag_embeddings
            .iter()
            .map(|record| record.source_key.as_str())
            .collect();
        pending.extend(
            tags.into_values()
                .filter(|input| !stored_tags.contains(input.source_key.as_str())),
        );
        let claims: HashMap<_, _> = tx
            .embedding_claims(&namespace, subject, &space.id)
            .await?
            .into_iter()
            .map(|claim| ((claim.kind, claim.source_key.clone()), claim))
            .collect();
        let mut jobs = BTreeSet::new();
        let mut fresh = Vec::new();
        let mut pending_count = 0;
        for input in pending {
            // The read snapshot can precede a completed worker; the writer now
            // serializes all admissions, so recheck accepted records and claims.
            if self
                .database
                .embedding_record(
                    &namespace,
                    subject,
                    &space.id,
                    input.kind,
                    &input.source_key,
                )
                .await?
                .is_some()
            {
                continue;
            }
            pending_count += 1;
            if let Some(claim) = claims.get(&(input.kind, input.source_key.clone())) {
                if claim.source_hash != input.source_hash {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "embedding input identity changed while pending",
                    ));
                }
                jobs.insert(claim.job_id.clone());
            } else {
                fresh.push(input);
            }
        }
        let mut batch = Vec::new();
        let mut bytes = 0;
        for input in fresh {
            if input.text.len() > config.max_input_bytes {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    format!("{} exceeds embedding max_input_bytes", input.source_key),
                ));
            }
            if !batch.is_empty()
                && (batch.len() >= config.max_batch_size
                    || bytes + input.text.len() > config.max_input_bytes)
            {
                jobs.insert(
                    queue_batch(
                        &mut tx,
                        &namespace,
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
            jobs.insert(queue_batch(&mut tx, &namespace, subject, &space, batch).await?);
        }
        tx.commit(json!({"subject_id":subject,"space":space,"pending_inputs":pending_count,"reused_vectors":reused,"jobs":jobs})).await
    }

    pub async fn get_embedding_run(&self, ctx: &CallContext, job: &str) -> Result<EmbeddingRun> {
        let run = self.database.embedding_run(job).await?;
        ctx.require_subject(&run.namespace_id, &run.subject_id)?;
        Ok(run)
    }

    pub(crate) async fn query_embedding(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        text: &str,
    ) -> Result<EmbeddingRecord> {
        let config = self.model.embedding_config().ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "text embedding resource is not configured",
            )
        })?;
        let space = config.space();
        let input = text_input(EmbeddingKind::Query, text);
        if input.text.is_empty() {
            return Err(Error::invalid("semantic query requires text"));
        }
        if let Some(record) = self
            .database
            .embedding_record(namespace, subject, &space.id, input.kind, &input.source_key)
            .await?
        {
            return Ok(record);
        }
        // Validate the complete request before admitting durable work.
        self.model
            .prepare_embedding(vec![input.clone()])
            .map_err(|failure| failure.to_domain_error())?;
        let scope = format!("namespace:{namespace}/subject:{subject}");
        let key = operation_key(
            ctx,
            "embedding.query",
            &scope,
            &ulid::Ulid::generate().to_string(),
            &json!({"space_id":space.id,"source_key":input.source_key}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::New(tx) => tx,
            BeginOperation::Replay(_) => unreachable!("new query admission"),
        };
        tx.save_embedding_space(&space).await?;
        let job = queue_batch(&mut tx, namespace, subject, &space, vec![input.clone()]).await?;
        tx.commit(json!({"job_id":job,"space_id":space.id})).await?;
        self.wait_embeddings(std::slice::from_ref(&job)).await?;
        self.database
            .embedding_record(namespace, subject, &space.id, input.kind, &input.source_key)
            .await?
            .ok_or_else(|| Error::internal("successful query embedding is missing"))
    }

    pub(crate) async fn wait_embeddings(&self, jobs: &[String]) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            let mut done = true;
            for job in jobs {
                let run = self.database.embedding_run(job).await?;
                match run.state {
                    EmbeddingState::Succeeded => {}
                    EmbeddingState::Failed
                    | EmbeddingState::Cancelled
                    | EmbeddingState::WaitingForConfiguration
                    | EmbeddingState::WaitingForBudget => {
                        return Err(run.error.unwrap_or_else(|| {
                            Error::new(
                                ErrorCode::DependencyUnavailable,
                                format!("embedding job {job} is {:?}", run.state),
                            )
                        }));
                    }
                    _ => done = false,
                }
            }
            if done {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorCode::DependencyUnavailable,
                    format!("embedding work is still pending; jobs: {}", jobs.join(",")),
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(crate) async fn process_embedding(&self, job: &str) -> Result<()> {
        let mut run = self.database.embedding_run(job).await?;
        if matches!(
            run.state,
            EmbeddingState::Succeeded | EmbeddingState::Failed | EmbeddingState::Cancelled
        ) {
            return Ok(());
        }
        if let Some(id) = &run.model_call_id {
            let call = self.database.model_call(id).await?;
            if call.state == ModelCallState::Succeeded {
                return self.adopt_embedding(&run, &call).await;
            }
            if call.state == ModelCallState::Started {
                self.mark_interrupted_call(call).await?;
            }
        }
        for input in &run.inputs {
            if let Some(id) = &input.memory_id {
                let memory = self.database.memory(&run.namespace_id, id).await;
                if !memory.as_ref().is_ok_and(|memory| {
                    !memory.deleted && memory_input(memory).source_hash == input.source_hash
                }) {
                    return self
                        .embedding_state(
                            &run,
                            EmbeddingState::Cancelled,
                            Error::new(
                                ErrorCode::Conflict,
                                "embedding source was revised or removed",
                            ),
                        )
                        .await;
                }
            }
        }
        let prepared = match self.model.prepare_embedding(run.inputs.clone()) {
            Ok(prepared) => prepared,
            Err(failure) => {
                return self
                    .embedding_state(
                        &run,
                        if failure.class() == RetryClass::WaitingConfiguration {
                            EmbeddingState::WaitingForConfiguration
                        } else {
                            EmbeddingState::Failed
                        },
                        failure.to_domain_error(),
                    )
                    .await;
            }
        };
        if prepared.metadata.space.id != run.space_id
            || prepared.metadata.producer_signature != run.producer_signature
        {
            return self
                .embedding_state(
                    &run,
                    EmbeddingState::Failed,
                    Error::new(
                        ErrorCode::Conflict,
                        "embedding producer changed; submit work for the configured space",
                    ),
                )
                .await;
        }
        let metadata = &prepared.metadata;
        let Some(reserved) = self
            .reserve_call(CallRequest {
                job_id: job,
                namespace_id: &run.namespace_id,
                subject_id: &run.subject_id,
                call_id: metadata.call_id.parse()?,
                resource_id: &metadata.resource.id,
                max_calls_per_day: metadata.max_calls_per_day,
                request: serde_json::to_value(metadata)?,
            })
            .await?
        else {
            return Ok(());
        };
        let mut tx = reserved.transaction;
        if tx.embedding_run(job).await?.state == EmbeddingState::Cancelled {
            tx.rollback().await?;
            return Ok(());
        }
        let Some(mut call) = reserved.call else {
            run.state = EmbeddingState::WaitingForBudget;
            run.error = Some(budget_error());
            run.updated_at = Utc::now().to_rfc3339();
            tx.update_embedding_run(&run).await?;
            tx.commit(json!({"state":run.state})).await?;
            return Ok(());
        };
        run.state = EmbeddingState::Running;
        run.error = None;
        run.model_call_id = Some(call.id.clone());
        run.updated_at = call.created_at.clone();
        tx.update_embedding_run(&run).await?;
        tx.commit(json!({"model_call_id":call.id})).await?;
        let execution = self.model.execute_embedding(prepared).await;
        let (vectors, failure) = match execution.result {
            Ok(vectors) => (Some(vectors), None),
            Err(error) => (None, Some(error)),
        };
        settle_call(
            &mut call,
            execution.record.completed_at.clone(),
            json!({"schema_version":1,"record":execution.record,"vectors":vectors}),
            failure.as_ref(),
        );
        if let Some(mut tx) = self
            .worker_transaction(
                "model.complete",
                call.id.as_str(),
                job,
                &json!({"call_id":call.id}),
            )
            .await?
        {
            let mut current = tx.embedding_run(job).await?;
            if current.state == EmbeddingState::Cancelled {
                let mut retired = tx.model_call(&call.id).await?;
                complete_redacted_call(&mut retired, &call);
                tx.update_model_call(&retired).await?;
                tx.commit(json!({"call_id":call.id,"state":retired.state,"source_retired":true}))
                    .await?;
                return Ok(());
            }
            tx.update_model_call(&call).await?;
            if let Some(error) = &failure {
                current.state = if error.class() == RetryClass::Permanent || call.attempt >= 5 {
                    EmbeddingState::Failed
                } else {
                    EmbeddingState::Queued
                };
                current.error = Some(error.to_domain_error());
            }
            current.updated_at = call.completed_at.clone().unwrap_or_default();
            tx.update_embedding_run(&current).await?;
            tx.commit(json!({"call_id":call.id,"state":call.state}))
                .await?;
            run = current;
        }
        if let Some(failure) = failure {
            if run.state == EmbeddingState::Failed {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(
                failure.info.retry_after_seconds.unwrap_or(2).min(30),
            ))
            .await;
            return Err(failure.to_domain_error());
        }
        self.adopt_embedding(&run, &call).await
    }

    async fn adopt_embedding(&self, run: &EmbeddingRun, call: &ModelCall) -> Result<()> {
        let Some(mut tx) = self
            .worker_transaction(
                "embedding.adopt",
                &run.job_id,
                &run.job_id,
                &json!({"call_id":call.id}),
            )
            .await?
        else {
            return Ok(());
        };
        let mut current = tx.embedding_run(&run.job_id).await?;
        if matches!(
            current.state,
            EmbeddingState::Succeeded | EmbeddingState::Cancelled
        ) {
            tx.rollback().await?;
            return Ok(());
        }
        let saved = tx.model_call(&call.id).await?;
        if saved.state != ModelCallState::Succeeded
            || current.model_call_id.as_ref() != Some(&saved.id)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding call is not available for adoption",
            ));
        }
        let vectors: Vec<Vec<f32>> = serde_json::from_value(
            saved
                .response
                .as_ref()
                .ok_or_else(|| Error::internal("missing embedding output"))?["vectors"]
                .clone(),
        )?;
        if vectors.len() != current.inputs.len() {
            return Err(Error::internal("embedding output count changed"));
        }
        for input in &current.inputs {
            if let Some(id) = &input.memory_id {
                match tx.memory(&run.namespace_id, id).await {
                    Ok(memory)
                        if !memory.deleted
                            && memory_input(&memory).source_hash == input.source_hash => {}
                    Ok(_)
                    | Err(Error {
                        code: ErrorCode::NotFound,
                        ..
                    }) => {
                        current.state = EmbeddingState::Cancelled;
                        current.error = Some(Error::new(
                            ErrorCode::Conflict,
                            "embedding source changed before adoption",
                        ));
                        current.updated_at = Utc::now().to_rfc3339();
                        tx.update_embedding_run(&current).await?;
                        tx.commit(json!({"state":current.state})).await?;
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let space: EmbeddingSpace = serde_json::from_value(saved.request["space"].clone())?;
        tx.save_embedding_space(&space).await?;
        for (input, vector) in current.inputs.iter().zip(vectors) {
            tx.save_embedding_record(&EmbeddingRecord {
                namespace_id: run.namespace_id.clone(),
                subject_id: run.subject_id.clone(),
                space_id: run.space_id.clone(),
                kind: input.kind,
                source_key: input.source_key.clone(),
                source_hash: input.source_hash.clone(),
                memory_id: input.memory_id.clone(),
                memory_revision: input.memory_revision,
                vector,
                model_call_id: call.id.clone(),
                commit_seq: tx.commit_seq(),
            })
            .await?;
        }
        current.state = EmbeddingState::Succeeded;
        current.error = None;
        current.updated_at = Utc::now().to_rfc3339();
        tx.update_embedding_run(&current).await?;
        tx.commit(
            json!({"job_id":run.job_id,"vectors":current.inputs.len(),"space_id":run.space_id}),
        )
        .await?;
        Ok(())
    }

    async fn embedding_state(
        &self,
        run: &EmbeddingRun,
        state: EmbeddingState,
        error: Error,
    ) -> Result<()> {
        if let Some(mut tx) = self
            .worker_transaction(
                "embedding.state",
                &ulid::Ulid::generate().to_string(),
                &run.job_id,
                &json!({"state":state,"error":error}),
            )
            .await?
        {
            let mut current = tx.embedding_run(&run.job_id).await?;
            if matches!(
                current.state,
                EmbeddingState::Succeeded | EmbeddingState::Cancelled
            ) {
                tx.rollback().await?;
                return Ok(());
            }
            current.state = state;
            current.error = Some(error);
            current.updated_at = Utc::now().to_rfc3339();
            tx.update_embedding_run(&current).await?;
            tx.commit(json!({"state":state})).await?;
        }
        Ok(())
    }
}

async fn queue_batch(
    tx: &mut WriteTransaction,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space: &EmbeddingSpace,
    inputs: Vec<EmbeddingInputItem>,
) -> Result<String> {
    for nonce in 0u64.. {
        let digest = Sha256::digest(serde_json::to_vec(&(
            "nous.embedding.job.v1",
            namespace,
            subject,
            &space.id,
            &inputs,
            nonce,
        ))?);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        let id = ulid::Ulid::from_bytes(bytes).to_string();
        match tx.embedding_run(&id).await {
            Ok(run)
                if matches!(
                    run.state,
                    EmbeddingState::Failed | EmbeddingState::Cancelled
                ) =>
            {
                continue;
            }
            Ok(_) => return Ok(id),
            Err(error) if error.code == ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let now = Utc::now().to_rfc3339();
        let run = EmbeddingRun {
            job_id: id.clone(),
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            producer_signature: space.producer_signature.clone(),
            space_id: space.id.clone(),
            inputs,
            state: EmbeddingState::Queued,
            model_call_id: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        tx.insert_embedding_run(&run).await?;
        tx.enqueue(JobEnvelope {
            id: id.clone(),
            kind: "embedding.text".into(),
            namespace_id: namespace.to_string(),
            subject_id: Some(subject.to_string()),
            expected_revision: None,
            payload: json!({"schema_version":1,"space_id":space.id}),
        })
        .await?;
        return Ok(id);
    }
    unreachable!("embedding job nonce exhausted")
}
