//! Stable concept identity, immutable revisions and explicit semantic lineage.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use sqlx::{Postgres, Transaction};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagContent {
    pub label: String,
    pub description: Option<String>,
    pub kind_hint: Option<String>,
}
impl TagContent {
    pub(crate) fn validate(&self) -> Result<()> {
        tag_semantic_representation(
            &self.label,
            self.description.as_deref(),
            self.kind_hint.as_deref(),
        )?;
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagExpectation {
    pub tag_id: TagId,
    pub expected_revision_id: Uuid,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseTagInput {
    pub operation_id: OperationId,
    pub target: TagExpectation,
    pub content: TagContent,
    pub producer: Option<ProducerSignature>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeTagsInput {
    pub operation_id: OperationId,
    pub survivor: TagExpectation,
    pub retired: Vec<TagExpectation>,
    pub basis: Vec<RevisionBasis>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitTagInput {
    pub producer: Option<ProducerSignature>,
    pub operation_id: OperationId,
    pub parent: TagExpectation,
    pub children: Vec<TagContent>,
    pub basis: Vec<RevisionBasis>,
}

impl MemoryService {
    pub async fn create_tag(&self, subject: SubjectId, mut input: CreateTagRequest) -> Result<Tag> {
        input.producer = input
            .producer
            .as_ref()
            .map(canonical_concept_producer)
            .transpose()?;
        let content = TagContent {
            label: input.label.clone(),
            description: input.description.clone(),
            kind_hint: input.kind_hint.clone(),
        };
        content.validate()?;
        let digest = operation_digest(
            "create_tag",
            subject,
            &serde_json::json!({"label":input.label,"description":input.description,"kind_hint":input.kind_hint,"origin":input.origin,"producer":input.producer}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, input.operation_id, "create_tag", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => return self.replay_tag(subject, receipt).await,
            MutationStart::Active(mutation) => mutation,
        };
        let tag = self
            .create_tag_in(
                mutation.tx(),
                subject,
                &content,
                &input.origin,
                input.producer.as_ref(),
            )
            .await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .publish_workflow_results("memory", &[CognitiveRef::Tag(tag.tag_id)])
            .await?;
        mutation
            .commit(
                "tag",
                Some(&tag.tag_id.0.to_string()),
                Some(tag.current_revision_id),
                None,
            )
            .await?;
        Ok(tag)
    }
    pub async fn revise_tag(&self, subject: SubjectId, mut input: ReviseTagInput) -> Result<Tag> {
        input.producer = input
            .producer
            .as_ref()
            .map(canonical_concept_producer)
            .transpose()?;
        input.content.validate()?;
        let digest = operation_digest("revise_tag", subject, &input)?;
        let mut mutation = match self
            .start_mutation(subject, input.operation_id, "revise_tag", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => return self.replay_tag(subject, receipt).await,
            MutationStart::Active(mutation) => mutation,
        };
        let revision_id = self
            .revise_tag_in(
                mutation.tx(),
                subject,
                &input.target,
                &input.content,
                input.producer.as_ref(),
            )
            .await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .publish_workflow_results("memory", &[CognitiveRef::Tag(input.target.tag_id)])
            .await?;
        mutation
            .commit(
                "tag",
                Some(&input.target.tag_id.0.to_string()),
                Some(revision_id),
                None,
            )
            .await?;
        let mut tag = self.tag(subject, input.target.tag_id).await?;
        tag.current_revision_id = revision_id;
        Ok(tag)
    }
    pub async fn merge_tags(&self, subject: SubjectId, input: MergeTagsInput) -> Result<Tag> {
        let digest = operation_digest("merge_tags", subject, &input)?;
        let mut mutation = match self
            .start_mutation(subject, input.operation_id, "merge_tags", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => return self.replay_tag(subject, receipt).await,
            MutationStart::Active(mutation) => mutation,
        };
        self.merge_tags_in(mutation.tx(), subject, &input).await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .publish_workflow_results("memory", &[CognitiveRef::Tag(input.survivor.tag_id)])
            .await?;
        mutation
            .commit(
                "tag",
                Some(&input.survivor.tag_id.0.to_string()),
                None,
                None,
            )
            .await?;
        self.tag(subject, input.survivor.tag_id).await
    }
    pub async fn split_tag(
        &self,
        subject: SubjectId,
        mut input: SplitTagInput,
    ) -> Result<Vec<Tag>> {
        input.producer = input
            .producer
            .as_ref()
            .map(canonical_concept_producer)
            .transpose()?;
        let digest = operation_digest("split_tag", subject, &input)?;
        let mut mutation = match self
            .start_mutation(subject, input.operation_id, "split_tag", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state != "committed" {
                    return Err(Error::Unavailable("tag operation in progress".into()));
                }
                let ids: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT child_tag_id,child_revision_id FROM tag_lineage WHERE subject_id=$1 AND operation_id=$2 AND relation='split_into' ORDER BY child_index")
                    .bind(subject.0).bind(input.operation_id.0).fetch_all(self.store.pool()).await.map_err(db)?;
                let mut result = Vec::new();
                for (id, revision) in ids {
                    let mut tag = self.tag(subject, TagId(id)).await?;
                    tag.current_revision_id = revision;
                    result.push(tag);
                }
                return Ok(result);
            }
            MutationStart::Active(mutation) => mutation,
        };
        let children = self
            .split_tag_in(mutation.tx(), subject, &input, input.producer.as_ref())
            .await?;
        mutation.invalidate(ProjectionInvalidation::all()).await?;
        mutation
            .publish_workflow_results(
                "memory",
                &children
                    .iter()
                    .map(|child| CognitiveRef::Tag(child.tag_id))
                    .collect::<Vec<_>>(),
            )
            .await?;
        mutation
            .commit(
                "tag_split",
                Some(&input.parent.tag_id.0.to_string()),
                None,
                None,
            )
            .await?;
        Ok(children)
    }
    async fn replay_tag(
        &self,
        subject: SubjectId,
        receipt: nous_persistence::MutationReceipt,
    ) -> Result<Tag> {
        if receipt.state != "committed" {
            return Err(Error::Unavailable("tag operation in progress".into()));
        }
        let id = receipt
            .result_ref
            .ok_or_else(|| Error::Infrastructure("tag receipt missing result".into()))?
            .parse()
            .map_err(|_| Error::Infrastructure("invalid Tag receipt".into()))?;
        let mut tag = self.tag(subject, TagId(id)).await?;
        if let Some(revision) = receipt.result_revision {
            tag.current_revision_id = revision;
        }
        Ok(tag)
    }
    pub(crate) async fn create_tag_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        content: &TagContent,
        origin: &str,
        producer: Option<&ProducerSignature>,
    ) -> Result<Tag> {
        content.validate()?;
        let tag_id = TagId::new();
        let revision_id = Uuid::now_v7();
        let now = AuthorityStore::authority_time_in(tx).await?;
        let producer_id = match producer {
            Some(p) => Some(AuthorityStore::register_producer_in(tx, p).await?),
            None => None,
        };
        sqlx::query("INSERT INTO tags(tag_id,subject_id,current_revision_id,created_at,status) VALUES($1,$2,$3,$4,'active')")
            .bind(tag_id.0).bind(subject.0).bind(revision_id).bind(now).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("INSERT INTO tag_revisions(tag_revision_id,tag_id,revision_no,label,description,kind_hint,origin,producer_signature_id,created_at) VALUES($1,$2,1,$3,$4,$5,$6,$7,$8)")
            .bind(revision_id).bind(tag_id.0).bind(&content.label).bind(&content.description).bind(&content.kind_hint).bind(origin).bind(producer_id).bind(now).execute(&mut **tx).await.map_err(db)?;
        self.sync_tag_identity_in(tx, subject, tag_id, &content.label)
            .await?;
        Ok(Tag {
            tag_id,
            subject_id: subject,
            current_revision_id: revision_id,
            status: "active".into(),
            canonical_tag_id: None,
            created_at: now,
        })
    }
    pub(crate) async fn revise_tag_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        target: &TagExpectation,
        content: &TagContent,
        producer: Option<&ProducerSignature>,
    ) -> Result<Uuid> {
        content.validate()?;
        self.expect_tag_in(tx, subject, target).await?;
        let revision_id = Uuid::now_v7();
        let producer_id = match producer {
            Some(p) => Some(AuthorityStore::register_producer_in(tx, p).await?),
            None => None,
        };
        sqlx::query("INSERT INTO tag_revisions(tag_revision_id,tag_id,revision_no,label,description,kind_hint,origin,producer_signature_id,created_at) SELECT $1,tag_id,revision_no+1,$2,$3,$4,origin,$5,$6 FROM tag_revisions WHERE tag_revision_id=$7")
            .bind(revision_id).bind(&content.label).bind(&content.description).bind(&content.kind_hint).bind(producer_id).bind(AuthorityStore::authority_time_in(tx).await?).bind(target.expected_revision_id).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("UPDATE tags SET current_revision_id=$3 WHERE subject_id=$1 AND tag_id=$2")
            .bind(subject.0)
            .bind(target.tag_id.0)
            .bind(revision_id)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        self.sync_tag_identity_in(tx, subject, target.tag_id, &content.label)
            .await?;
        Ok(revision_id)
    }
    pub(crate) async fn expect_tag_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        target: &TagExpectation,
    ) -> Result<()> {
        let row = sqlx::query("SELECT current_revision_id,status FROM tags WHERE subject_id=$1 AND tag_id=$2 FOR UPDATE")
            .bind(subject.0).bind(target.tag_id.0).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(|| Error::NotFound("Tag outside Subject".into()))?;
        if row.try_get::<Uuid, _>("current_revision_id").map_err(db)? != target.expected_revision_id
            || row.try_get::<String, _>("status").map_err(db)? != "active"
        {
            return Err(DomainError::new(
                DomainErrorCode::StaleRevision,
                "Tag head or status changed",
            )
            .with_context("tag_id", target.tag_id.0)
            .with_context("expected_revision_id", target.expected_revision_id)
            .into());
        }
        Ok(())
    }
    pub(crate) async fn merge_tags_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        input: &MergeTagsInput,
    ) -> Result<()> {
        if input.retired.is_empty() || input.retired.len() > 7 {
            return Err(Error::Invalid("merge requires 1..7 retired Tags".into()));
        }
        let mut ids = HashSet::from([input.survivor.tag_id]);
        if input.retired.iter().any(|tag| !ids.insert(tag.tag_id)) {
            return Err(Error::Invalid("duplicate merge Tag".into()));
        }
        self.validate_tag_lineage_in(tx, subject, &input.basis)
            .await?;
        self.expect_tag_in(tx, subject, &input.survivor).await?;
        for (index, tag) in input.retired.iter().enumerate() {
            self.expect_tag_in(tx, subject, tag).await?;
            sqlx::query("UPDATE tags SET canonical_tag_id=$3,status='merged' WHERE subject_id=$1 AND (tag_id=$2 OR canonical_tag_id=$2)")
                .bind(subject.0).bind(tag.tag_id.0).bind(input.survivor.tag_id.0).execute(&mut **tx).await.map_err(db)?;
            self.record_tag_lineage_in(
                tx,
                subject,
                input.operation_id,
                tag.tag_id,
                input.survivor.tag_id,
                index,
                "merged_into",
                &input.basis,
            )
            .await?;
        }
        sqlx::query("UPDATE lexical_visibility v SET aliases=ARRAY(SELECT DISTINCT a FROM (SELECT unnest(v.aliases) a UNION ALL SELECT rv.display_name FROM lexical_bindings rb JOIN lexical_visibility rv USING(lexical_ref) WHERE rv.subject_id=$1 AND rb.object_kind='tag' AND canonical_tag($1,CASE WHEN rb.object_kind='tag' THEN rb.canonical_ref::uuid END)=$2 UNION ALL SELECT unnest(rv.aliases) FROM lexical_bindings rb JOIN lexical_visibility rv USING(lexical_ref) WHERE rv.subject_id=$1 AND rb.object_kind='tag' AND canonical_tag($1,CASE WHEN rb.object_kind='tag' THEN rb.canonical_ref::uuid END)=$2) names WHERE a<>v.display_name ORDER BY a) FROM lexical_bindings b WHERE v.subject_id=$1 AND v.lexical_ref=b.lexical_ref AND b.object_kind='tag' AND b.canonical_ref=$2::uuid::text")
            .bind(subject.0).bind(input.survivor.tag_id.0).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }
    pub(crate) async fn split_tag_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        input: &SplitTagInput,
        producer: Option<&ProducerSignature>,
    ) -> Result<Vec<Tag>> {
        if !(2..=8).contains(&input.children.len()) {
            return Err(Error::Invalid("split requires 2..8 child concepts".into()));
        }
        let mut labels = HashSet::new();
        for child in &input.children {
            child.validate()?;
            if !labels.insert(child.label.trim().to_lowercase()) {
                return Err(Error::Invalid("duplicate split concept".into()));
            }
        }
        self.validate_tag_lineage_in(tx, subject, &input.basis)
            .await?;
        self.expect_tag_in(tx, subject, &input.parent).await?;
        let mut result = Vec::new();
        for (index, content) in input.children.iter().enumerate() {
            let child = self
                .create_tag_in(tx, subject, content, "split", producer)
                .await?;
            self.record_tag_lineage_in(
                tx,
                subject,
                input.operation_id,
                input.parent.tag_id,
                child.tag_id,
                index,
                "split_into",
                &input.basis,
            )
            .await?;
            result.push(child);
        }
        Ok(result)
    }
    async fn validate_tag_lineage_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        if !(1..=16).contains(&basis.len()) {
            return Err(Error::Invalid("Tag lineage needs 1..16 exact basis".into()));
        }
        let mut keys = HashSet::new();
        for basis in basis {
            if matches!(basis, RevisionBasis::Seed(_)) || !keys.insert(basis.canonical_key()) {
                return Err(Error::Invalid(
                    "invalid or duplicate Tag lineage support".into(),
                ));
            }
        }
        self.validate_basis_for_subject(subject, basis).await?;
        self.validate_basis_in_tx(tx, subject, basis).await?;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "typed lineage fact has two endpoints and operation provenance"
    )]
    async fn record_tag_lineage_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        operation: OperationId,
        parent: TagId,
        child: TagId,
        index: usize,
        relation: &str,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        sqlx::query("INSERT INTO tag_lineage(lineage_id,subject_id,operation_id,parent_tag_id,child_tag_id,parent_revision_id,child_revision_id,child_index,relation,basis,created_at) SELECT $1,$2,$3,$4,$5,p.current_revision_id,c.current_revision_id,$6,$7,$8,$9 FROM tags p JOIN tags c ON c.subject_id=p.subject_id WHERE p.subject_id=$2 AND p.tag_id=$4 AND c.tag_id=$5")
            .bind(Uuid::now_v7()).bind(subject.0).bind(operation.0).bind(parent.0).bind(child.0).bind(i32::try_from(index).map_err(|_| Error::Invalid("lineage index exceeded".into()))?).bind(relation).bind(serde_json::to_value(basis).map_err(|e| Error::Infrastructure(e.to_string()))?).bind(AuthorityStore::authority_time_in(tx).await?).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }
    async fn sync_tag_identity_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        tag: TagId,
        label: &str,
    ) -> Result<()> {
        let previous: Option<String> = sqlx::query_scalar("SELECT v.display_name FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref) WHERE b.object_kind='tag' AND b.canonical_ref=$2 AND v.subject_id=$1")
            .bind(subject.0).bind(tag.0.to_string()).fetch_optional(&mut **tx).await.map_err(db)?;
        let binding = self
            .store
            .bind_identity_in(tx, subject, CognitiveRef::Tag(tag), label.into(), vec![])
            .await?;
        if let Some(previous) = previous.filter(|old| old != label) {
            sqlx::query("UPDATE lexical_visibility SET aliases=ARRAY(SELECT DISTINCT alias FROM unnest(aliases || ARRAY[$3]::text[]) alias ORDER BY alias) WHERE subject_id=$1 AND lexical_ref=$2")
                .bind(subject.0).bind(binding.lexical_ref).bind(previous).execute(&mut **tx).await.map_err(db)?;
        }
        Ok(())
    }
    pub(in crate::service) async fn tag(&self, subject: SubjectId, tag_id: TagId) -> Result<Tag> {
        let row = sqlx::query("SELECT tag_id,subject_id,current_revision_id,canonical_tag_id,created_at,status FROM tags WHERE subject_id=$1 AND tag_id=$2")
            .bind(subject.0).bind(tag_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.ok_or_else(|| Error::NotFound("Tag not found".into()))?;
        Ok(Tag {
            tag_id: TagId(row.try_get("tag_id").map_err(db)?),
            subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
            current_revision_id: row.try_get("current_revision_id").map_err(db)?,
            status: row.try_get("status").map_err(db)?,
            canonical_tag_id: row
                .try_get::<Option<Uuid>, _>("canonical_tag_id")
                .map_err(db)?
                .map(TagId),
            created_at: row.try_get("created_at").map_err(db)?,
        })
    }
}

fn canonical_concept_producer(producer: &ProducerSignature) -> Result<ProducerSignature> {
    if producer.operation != CapabilityOperation::ConceptMaintenanceText {
        return Err(Error::Invalid("concept producer operation mismatch".into()));
    }
    AuthorityStore::canonical_producer(producer)
}
