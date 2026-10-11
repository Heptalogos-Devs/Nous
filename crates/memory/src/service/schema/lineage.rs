// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn split_schemas(
        &self,
        subject: SubjectId,
        schema_id: CognitiveSchemaId,
        operation_id: OperationId,
        expected_object_epoch: i64,
        mut children: Vec<CreateSchemaInput>,
    ) -> Result<Vec<MutationResult>> {
        if children.len() < 2 || children.len() > 16 {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "schema split needs 2..16 children",
            ));
        }
        for child in &mut children {
            child.content.producer = canonical_schema_producer(child.content.producer.as_ref())?;
            validate_schema_content(&child.content)?;
        }
        let digest = operation_digest(
            "split_cognitive_schema",
            subject,
            &serde_json::json!({"schema_id":schema_id,"expected_object_epoch":expected_object_epoch,"children":children}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Concept,
                subject,
                operation_id,
                "split_cognitive_schema",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => return Ok(receipt.results),
            MutationStart::Active(mutation) => mutation,
        };
        let source=sqlx::query("SELECT current_revision_id,object_epoch,acceptance_state,purge_state FROM cognition_objects WHERE subject_id=$1 AND object_kind='cognitive_schema' AND object_id=$2 FOR UPDATE").bind(subject.0).bind(schema_id.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::new(nous_core::ErrorOwner::Concept, nous_core::ErrorCode::NotFound, "CognitiveSchema not found"))?;
        if source
            .try_get::<String, _>("acceptance_state")
            .map_err(db)?
            != "accepted"
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::FailedPrecondition,
                "source CognitiveSchema is withdrawn",
            ));
        }
        if source.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::FailedPrecondition,
                "source CognitiveSchema is purging",
            ));
        }
        let epoch: i64 = source.try_get("object_epoch").map_err(db)?;
        crate::service::state::fence_epoch(ErrorOwner::Concept, epoch, expected_object_epoch)?;
        let source_revision: Uuid = source.try_get("current_revision_id").map_err(db)?;
        let mut ids = Vec::new();
        let mut revisions = Vec::new();
        for mut child in children {
            child.subject = subject;
            for link in &child.content.evidence_links {
                self.validate_basis_for_subject(
                    ErrorOwner::Concept,
                    subject,
                    std::slice::from_ref(&link.basis),
                )
                .await?;
            }
            self.validate_schema_formation(subject, &child.content)
                .await?;
            let producer = if let Some(value) = &child.content.producer {
                Some(AuthorityStore::register_producer_in(mutation.tx(), value).await?)
            } else {
                None
            };
            let (child_id, child_revision) = self
                .create_schema_in(mutation.tx(), &child, producer, self.cognition.now(subject))
                .await?;
            sqlx::query("INSERT INTO cognitive_schema_lineage(from_revision_id,to_revision_id,relation) VALUES($1,$2,'schema_split_from')").bind(child_revision.0).bind(source_revision).execute(&mut **mutation.tx()).await.map_err(db)?;
            ids.push(child_id);
            revisions.push(child_revision);
        }
        sqlx::query("UPDATE cognition_objects SET acceptance_state='withdrawn',object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind='cognitive_schema' AND object_id=$2").bind(subject.0).bind(schema_id.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        for child in &ids {
            self.enqueue_concept_in(
                mutation.tx(),
                subject,
                CognitiveRef::CognitiveSchema(*child),
                sequence,
            )
            .await?;
        }
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "schema",
            &[schema_id.0],
            sequence,
            "source_split",
        )
        .await?;
        mutation
            .publish_workflow_results(
                "memory",
                &revisions
                    .iter()
                    .copied()
                    .map(CognitiveRef::CognitiveSchemaRevision)
                    .collect::<Vec<_>>(),
            )
            .await?;
        let mut results = vec![MutationResult::new(
            CognitiveRef::CognitiveSchema(schema_id),
            None,
            Some(epoch + 1),
        )];
        results.extend(revisions.iter().copied().map(|revision| {
            MutationResult::reference(CognitiveRef::CognitiveSchemaRevision(revision))
        }));
        mutation.commit(&results).await?;
        Ok(results)
    }

    pub async fn merge_schemas(
        &self,
        subject: SubjectId,
        operation_id: OperationId,
        source_ids: Vec<CognitiveSchemaId>,
        expected_epochs: Vec<i64>,
        mut merged: CreateSchemaInput,
    ) -> Result<MutationResult> {
        if source_ids.len() < 2
            || source_ids.len() > 16
            || source_ids.len() != expected_epochs.len()
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "schema merge needs 2..16 sources and matching epochs",
            ));
        }
        let unique_sources = source_ids.iter().collect::<std::collections::BTreeSet<_>>();
        if unique_sources.len() != source_ids.len() {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "schema merge sources must be distinct",
            ));
        }
        self.store.require_subject(subject).await?;
        merged.content.producer = canonical_schema_producer(merged.content.producer.as_ref())?;
        validate_schema_content(&merged.content)?;
        let digest = operation_digest(
            "merge_cognitive_schemas",
            subject,
            &serde_json::json!({"sources":source_ids,"epochs":expected_epochs,"merged":merged}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Concept,
                subject,
                operation_id,
                "merge_cognitive_schemas",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                return single_mutation_result(receipt, ErrorOwner::Concept);
            }
            MutationStart::Active(mutation) => mutation,
        };
        let mut sorted = source_ids.iter().copied().enumerate().collect::<Vec<_>>();
        sorted.sort_by_key(|(_, id)| *id);
        let mut source_revisions = Vec::with_capacity(sorted.len());
        for (index, id) in sorted {
            let row=sqlx::query("SELECT object_epoch,current_revision_id,acceptance_state,purge_state FROM cognition_objects WHERE subject_id=$1 AND object_kind='cognitive_schema' AND object_id=$2 FOR UPDATE").bind(subject.0).bind(id.0).fetch_one(&mut **mutation.tx()).await.map_err(db)?;
            if row.try_get::<String, _>("acceptance_state").map_err(db)? != "accepted" {
                return Err(Error::new(
                    nous_core::ErrorOwner::Concept,
                    nous_core::ErrorCode::FailedPrecondition,
                    "schema merge source is withdrawn",
                ));
            }
            if row.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
                return Err(Error::new(
                    nous_core::ErrorOwner::Concept,
                    nous_core::ErrorCode::FailedPrecondition,
                    "schema merge source is purging",
                ));
            }
            let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
            fence_epoch(ErrorOwner::Concept, epoch, expected_epochs[index])?;
            source_revisions.push((
                id,
                CognitiveSchemaRevisionId(row.try_get("current_revision_id").map_err(db)?),
            ));
        }
        let mut merged_links = std::collections::BTreeMap::new();
        for (_, revision) in &source_revisions {
            for link in active_schema_link_inputs(mutation.tx(), subject, *revision).await? {
                let key = format!("{}|{}", link.role.as_str(), link.basis.canonical_key());
                merged_links.entry(key).or_insert(link);
            }
        }
        for link in merged.content.evidence_links.clone() {
            let key = format!("{}|{}", link.role.as_str(), link.basis.canonical_key());
            merged_links.entry(key).or_insert(link);
        }
        if merged_links.len() < 2 {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "merged CognitiveSchema needs at least two active evidence links",
            ));
        }
        for link in merged_links.values() {
            self.validate_basis_for_subject(
                ErrorOwner::Concept,
                subject,
                std::slice::from_ref(&link.basis),
            )
            .await?;
        }
        merged.subject = subject;
        merged.content.evidence_links = merged_links.into_values().collect();
        self.validate_schema_formation(subject, &merged.content)
            .await?;
        let producer = if let Some(value) = &merged.content.producer {
            Some(AuthorityStore::register_producer_in(mutation.tx(), value).await?)
        } else {
            None
        };
        let (new_id, new_revision) = self
            .create_schema_in(
                mutation.tx(),
                &merged,
                producer,
                self.cognition.now(subject),
            )
            .await?;
        for (id, source_revision) in source_revisions {
            sqlx::query("UPDATE cognition_objects SET acceptance_state='withdrawn',object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind='cognitive_schema' AND object_id=$2").bind(subject.0).bind(id.0).execute(&mut **mutation.tx()).await.map_err(db)?;
            sqlx::query("INSERT INTO cognitive_schema_lineage(from_revision_id,to_revision_id,relation) VALUES($1,$2,'schema_merged_from')").bind(new_revision.0).bind(source_revision.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        }
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            subject,
            CognitiveRef::CognitiveSchemaRevision(new_revision),
            sequence,
        )
        .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "schema",
            &source_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
            sequence,
            "source_merged",
        )
        .await?;
        mutation
            .publish_workflow_results(
                "memory",
                &[
                    CognitiveRef::CognitiveSchema(new_id),
                    CognitiveRef::CognitiveSchemaRevision(new_revision),
                ],
            )
            .await?;
        let result = MutationResult::revised(
            CognitiveRef::CognitiveSchema(new_id),
            new_revision.0,
            Some(1),
        );
        mutation.commit(std::slice::from_ref(&result)).await?;
        Ok(result)
    }
}
