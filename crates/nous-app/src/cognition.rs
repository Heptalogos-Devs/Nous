use super::*;
use nous_retrieval::{IndexDocument, match_score};
use nous_storage::JobEnvelope;
use std::{collections::HashMap, time::Duration};

impl App {
    pub async fn form_memory(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject_id: &SubjectId,
        observation_id: &ObservationId,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.subject_namespace(subject_id).await?;
        ctx.require_subject(&namespace, subject_id)?;
        let subject = self.database.subject(&namespace, subject_id).await?;
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let observation = self
            .database
            .observation(&namespace, observation_id)
            .await?;
        if observation.subject_id != *subject_id {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "observation belongs to another subject",
            ));
        }
        let material = self
            .database
            .material(&namespace, &observation.material_id)
            .await?;
        let scope = format!("namespace:{namespace}/subject:{subject_id}");
        let key = operation_key(
            ctx,
            "memory.form",
            &scope,
            operation_id,
            &json!({"subject_id":subject_id,"observation_id":observation_id}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let current = tx.subject(&namespace, subject_id).await?;
        tx.observation(&namespace, observation_id).await?;
        tx.material(&namespace, &material.id).await?;
        if current.revision != subject.revision {
            return Err(Error::new(
                ErrorCode::Conflict,
                "subject changed during submission",
            ));
        }
        let job_id = ulid::Ulid::generate().to_string();
        let now = Utc::now().to_rfc3339();
        let run = FormationRun {
            job_id: job_id.clone(),
            namespace_id: namespace.clone(),
            subject_id: subject_id.clone(),
            observation_id: observation_id.clone(),
            material_id: material.id,
            subject_revision: subject.revision,
            material_hash: material.content_hash,
            state: FormationState::Queued,
            model_call_id: None,
            memory_ids: vec![],
            error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        tx.insert_formation(&run).await?;
        tx.enqueue(JobEnvelope {
            id: job_id,
            kind: "memory.form".into(),
            namespace_id: namespace.to_string(),
            subject_id: Some(subject_id.to_string()),
            expected_revision: Some(subject.revision),
            payload: json!({"schema_version":1,"observation_id":observation_id}),
        })
        .await?;
        tx.commit(serde_json::to_value(run)?).await
    }

    pub async fn get_memory(&self, ctx: &CallContext, id: &MemoryId) -> Result<Memory> {
        let namespace = self.database.memory_namespace(id).await?;
        let memory = self.database.memory(&namespace, id).await?;
        ctx.require_subject(&namespace, &memory.subject_id)?;
        if memory.deleted {
            return Err(Error::new(ErrorCode::NotFound, "memory is deleted"));
        }
        Ok(memory)
    }

    pub async fn memory_history(&self, ctx: &CallContext, id: &MemoryId) -> Result<Vec<Memory>> {
        let namespace = self.database.memory_namespace(id).await?;
        let memory = self.database.memory(&namespace, id).await?;
        ctx.require_subject(&namespace, &memory.subject_id)?;
        self.database.memory_history(&namespace, id).await
    }

    pub async fn delete_memory(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &MemoryId,
        expected_revision: i64,
    ) -> Result<OperationReceipt> {
        self.apply_memory_change(
            ctx,
            operation_id,
            id,
            MemoryChange {
                action: MemoryAction::Trash,
                expected_revision,
                strength: None,
                hours: None,
                reason: String::new(),
                force: false,
                edit: None,
            },
            "memory.delete",
            json!({"memory_id":id,"expected_revision":expected_revision}),
        )
        .await
    }

    pub async fn memory_source(&self, ctx: &CallContext, id: &MemoryId) -> Result<MemorySource> {
        let memory = self.get_memory(ctx, id).await?;
        let observation = self
            .database
            .observation(&memory.namespace_id, &memory.observation_id)
            .await?;
        let material = self.get_material(ctx, &memory.material_id).await?;
        let model_call = match &memory.model_call_id {
            Some(id) => Some(self.database.model_call(id).await?),
            None => None,
        };
        let embedding_provenance = self
            .database
            .embeddings_for_memory(&memory.namespace_id, &memory.id)
            .await?
            .iter()
            .map(crate::embeddings::provenance)
            .collect();
        Ok(MemorySource {
            memory,
            observation,
            material,
            model_call,
            embedding_provenance,
        })
    }

    pub async fn model_status(&self, ctx: &CallContext) -> Result<Value> {
        ctx.require_admin()?;
        let mut status = self.model.inspect();
        status["text_embedding"] = self.model.embedding_inspect();
        status["structured_formation"] = self.model.structured_inspect();
        Ok(status)
    }

    pub async fn query(&self, ctx: &CallContext, request: QueryRequest) -> Result<QueryResult> {
        if request.limit == 0 || request.limit > 100 || request.text.len() > 4096 {
            return Err(Error::invalid(
                "query limit must be 1..100 and text at most 4096 bytes",
            ));
        }
        if let Some(filter) = &request.time {
            validate_name(&filter.range.domain, "time domain")?;
            if matches!((filter.range.start,filter.range.end),(Some(start),Some(end)) if start>end)
            {
                return Err(Error::invalid("time range start is after its end"));
            }
        }
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&namespace, &request.subject_id)?;
        let latest = self.database.status().await?.commit_seq;
        if let Some(required) = request.min_commit_seq {
            if required < 0 || required > latest {
                return Err(Error::invalid(
                    "min_commit_seq must refer to a committed operation",
                ));
            }
            let started = tokio::time::Instant::now();
            while self
                .retrieval
                .coverage()
                .map_err(index_error)?
                .through_commit_seq
                < required
                && started.elapsed() < Duration::from_secs(3)
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        if request.mode != QueryMode::Lexical && request.reference.is_none() {
            return self.semantic_query(ctx, &namespace, request).await;
        }
        self.lexical_query(&namespace, request).await
    }

    pub(crate) async fn lexical_query(
        &self,
        namespace: &NamespaceId,
        request: QueryRequest,
    ) -> Result<QueryResult> {
        let text = request.text.clone();
        let subject_id = request.subject_id.clone();
        let ns = namespace.clone();
        let index = self.retrieval.clone();
        let candidate_limit = (request.limit * 8).min(800);
        let page = tokio::task::spawn_blocking(move || {
            index.search(&text, ns.as_str(), subject_id.as_str(), candidate_limit)
        })
        .await
        .map_err(|error| Error::internal(error.to_string()))?
        .map_err(index_error)?;
        let delta = self
            .database
            .subject_memory_changes(
                namespace,
                &request.subject_id,
                page.coverage.through_commit_seq,
                500,
            )
            .await?;
        let mut candidates: HashMap<MemoryId, (Memory, f32)> = HashMap::new();
        if let Some(id) = &request.reference {
            let memory = self.database.memory(namespace, id).await?;
            if memory.subject_id != request.subject_id {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "memory belongs to another subject",
                ));
            }
            candidates.insert(memory.id.clone(), (memory, 1.0));
        } else {
            for candidate in page.hits {
                let id: MemoryId = candidate.id.parse()?;
                let memory = match self.database.memory(namespace, &id).await {
                    Ok(memory) => memory,
                    Err(error) if error.code == ErrorCode::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if memory.revision == candidate.revision && memory.subject_id == request.subject_id
                {
                    candidates.insert(id, (memory, candidate.score));
                }
            }
            for memory in &delta.memories {
                candidates.remove(&memory.id);
                if !memory.deleted
                    && let Some(score) = match_score(&request.text, &memory_text(memory))
                {
                    candidates.insert(memory.id.clone(), (memory.clone(), score));
                }
            }
            for purged in &delta.purged {
                candidates.remove(&purged.id);
            }
        }
        let mut hits = Vec::new();
        for (id, (selected, mut score)) in candidates {
            let memory = match self.database.memory(namespace, &id).await {
                Ok(memory) => memory,
                Err(error) if error.code == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            if request.reference.is_none() && memory.revision != selected.revision {
                let Some(current_score) = match_score(&request.text, &memory_text(&memory)) else {
                    continue;
                };
                score = current_score;
            }
            if memory.deleted
                || memory.subject_id != request.subject_id
                || (!request.include_inactive
                    && request.reference.is_none()
                    && memory.lifecycle.activation != MemoryActivation::Active)
                || !matches_time(&memory, request.time.as_ref())
            {
                continue;
            }
            let material = self
                .database
                .material(namespace, &memory.material_id)
                .await?;
            if request
                .source
                .as_ref()
                .is_some_and(|source| material.source.as_ref() != Some(source))
            {
                continue;
            }
            hits.push(QueryHit {
                reference: format!("memory:{}@{}", memory.id, memory.revision),
                material_id: memory.material_id.clone(),
                observation_id: memory.observation_id.clone(),
                source: material.source,
                memory,
                score,
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.memory.commit_seq.cmp(&a.memory.commit_seq))
                .then_with(|| a.memory.id.as_str().cmp(b.memory.id.as_str()))
        });
        hits.truncate(request.limit);
        Ok(QueryResult {
            semantic: None,
            hits,
            coverage: QueryCoverage {
                indexed_through: page.coverage.through_commit_seq,
                authority_commit_seq: delta.through_commit_seq,
                complete: request.reference.is_some() || !delta.has_more,
                generation: page.coverage.generation,
            },
        })
    }

    pub(crate) async fn catch_up_index(&self) -> Result<()> {
        let _guard = self.index_gate.lock().await;
        // A single index owns atomic commit+watermark. Concurrent tasks can safely
        // repeat the same snapshot; later watermark commits supersede old ones.
        loop {
            let covered = self
                .retrieval
                .coverage()
                .map_err(index_error)?
                .through_commit_seq;
            let batch = self.database.memory_changes(covered, 500).await?;
            if batch.through_commit_seq <= covered {
                return Ok(());
            }
            let mut docs = batch
                .memories
                .iter()
                .map(|memory| IndexDocument {
                    id: memory.id.to_string(),
                    namespace_id: memory.namespace_id.to_string(),
                    subject_id: memory.subject_id.to_string(),
                    revision: memory.revision,
                    commit_seq: memory.commit_seq,
                    text: memory_text(memory),
                    deleted: memory.deleted,
                })
                .collect::<Vec<_>>();
            docs.extend(batch.purged.iter().map(|purged| IndexDocument {
                id: purged.id.to_string(),
                namespace_id: purged.namespace_id.to_string(),
                subject_id: purged.subject_id.to_string(),
                revision: purged.revision,
                commit_seq: purged.commit_seq,
                text: String::new(),
                deleted: true,
            }));
            let index = self.retrieval.clone();
            tokio::task::spawn_blocking(move || index.apply(&docs, batch.through_commit_seq))
                .await
                .map_err(|error| Error::internal(error.to_string()))?
                .map_err(index_error)?;
            if !batch.has_more {
                return Ok(());
            }
        }
    }
}

pub(crate) fn utc_point() -> TimePoint {
    TimePoint {
        domain: "unix_ms".into(),
        coordinate: Utc::now().timestamp_millis(),
    }
}
pub(crate) fn index_job(memory: &Memory) -> JobEnvelope {
    JobEnvelope {
        id: ulid::Ulid::generate().to_string(),
        kind: "memory.index".into(),
        namespace_id: memory.namespace_id.to_string(),
        subject_id: Some(memory.subject_id.to_string()),
        expected_revision: Some(memory.revision),
        payload: json!({"schema_version":1,"memory_id":memory.id,"commit_seq":memory.commit_seq}),
    }
}
fn memory_text(memory: &Memory) -> String {
    format!("{}\n{}", memory.title, memory.content)
}
fn index_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::DependencyUnavailable,
        format!("lexical index: {error}"),
    )
}
pub(crate) fn matches_time(memory: &Memory, filter: Option<&TimeFilter>) -> bool {
    matches_times(&memory.times, filter)
}
pub(crate) fn matches_times(times: &CognitiveTimes, filter: Option<&TimeFilter>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if matches!(filter.axis, TimeAxis::Validity) {
        return times.validity.as_ref().is_some_and(|range| {
            range.domain == filter.range.domain
                && !matches!((range.end,filter.range.start),(Some(end),Some(start)) if end<start)
                && !matches!((range.start,filter.range.end),(Some(start),Some(end)) if start>end)
        });
    }
    let point = match filter.axis {
        TimeAxis::Event => times.event.as_ref(),
        TimeAxis::Observation => times.observation.as_ref(),
        TimeAxis::Formation => Some(&times.formation),
        TimeAxis::Recorded => Some(&times.recorded),
        TimeAxis::Validity => None,
    };
    point.is_some_and(|point| {
        point.domain == filter.range.domain
            && filter
                .range
                .start
                .is_none_or(|start| point.coordinate >= start)
            && filter.range.end.is_none_or(|end| point.coordinate <= end)
    })
}
