//! Portable vectors retain their actual values, call proof and accepted source parts.
use crate::{ReadView, WriteTransaction, storage_error, write_error};
use nous_domain::*;
use sqlx::Row;
use std::collections::BTreeSet;

impl ReadView {
    pub(crate) async fn bundle_vector_snapshot(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<BundleVectors> {
        let mut output = BundleVectors::default();
        let mut spaces = BTreeSet::new();
        let rows=sqlx::query("SELECT space_id,source_hash FROM embedding_contents WHERE namespace_id=? AND subject_id=? ORDER BY space_id,source_hash").bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let space: String = row.get("space_id");
            let hash: String = row.get("source_hash");
            let content = self
                .matching_embedding(namespace, subject, &space, &hash)
                .await?
                .ok_or_else(crate::not_found)?;
            if spaces.insert(space.clone()) {
                output
                    .spaces
                    .push(super::runs::space(&mut self.tx, &space).await?);
            }
            output.contents.push(content);
        }
        let rows=sqlx::query("SELECT r.*,c.vector,c.model_call_id,s.parts,s.read_requirements,s.origin,s.retired_commit_seq,json_extract(o.receipt,'$.context') AS accepted_context,json_extract(o.receipt,'$.time') AS accepted_time FROM embedding_records r JOIN embedding_contents c USING(namespace_id,subject_id,space_id,source_hash) JOIN embedding_subscriptions s ON s.namespace_id=r.namespace_id AND s.subject_id=r.subject_id AND s.space_id=r.space_id AND s.source_hash=r.source_hash AND s.source_key=r.source_key AND s.accepted_commit_seq=r.accepted_commit_seq JOIN operation_receipts o ON o.commit_seq=s.accepted_commit_seq WHERE r.namespace_id=? AND r.subject_id=? ORDER BY r.space_id,r.source_key,r.source_hash")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let record = super::records::row_ref(&row)?;
            let input = EmbeddingInputItem {
                target: record.target.clone(),
                parts: serde_json::from_str(row.get("parts"))?,
                read_requirements: serde_json::from_str(row.get("read_requirements"))?,
            };
            let parent: Option<EmbeddingAcceptance> = row
                .get::<Option<&str>, _>("origin")
                .map(serde_json::from_str)
                .transpose()?;
            output.associations.push(BundleEmbeddingAssociation {
                record,
                input,
                acceptance: EmbeddingAcceptance {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                    context: serde_json::from_str(row.get("accepted_context"))?,
                    time: OperationTime::parse(row.get("accepted_time"))?,
                    commit_seq: row.get("accepted_commit_seq"),
                    parent: parent.map(Box::new),
                },
                retired: row.get::<Option<i64>, _>("retired_commit_seq").is_some(),
            });
        }
        Ok(output)
    }
}
impl WriteTransaction {
    pub async fn import_embedding_association(
        &mut self,
        value: &BundleEmbeddingAssociation,
    ) -> Result<()> {
        let record = &value.record;
        let input = &value.input;
        self.check_sequence(record.commit_seq)?;
        input.validate()?;
        if record.target != input.target || record.source_hash != input.source_hash() {
            return Err(Error::invalid(
                "imported vector association differs from its exact parts",
            ));
        }
        let context = self.context().clone();
        let mut own = input.clone();
        own.read_requirements.clear();
        self.require_embedding_source_read(
            &context,
            &record.namespace_id,
            &record.subject_id,
            &own,
        )
        .await?;
        self.validate_embedding_input(&record.namespace_id, &record.subject_id, input)
            .await?;
        let unavailable = self
            .require_source_requirements(
                &context,
                &record.namespace_id,
                &record.subject_id,
                &input.read_requirements,
            )
            .await;
        let retired = value.retired
            || match unavailable {
                Ok(()) => false,
                Err(error) if matches!(error.code, ErrorCode::NotFound | ErrorCode::Forbidden) => {
                    true
                }
                Err(error) => return Err(error),
            };
        let content = self
            .matching_embedding(
                &record.namespace_id,
                &record.subject_id,
                &record.space_id,
                &record.source_hash,
            )
            .await?
            .ok_or_else(crate::not_found)?;
        if content.model_call_id != record.model_call_id || content.vector != record.vector {
            return Err(Error::new(
                ErrorCode::Conflict,
                "imported target must use the accepted actual vector and call",
            ));
        }
        sqlx::query("INSERT INTO embedding_subscriptions(namespace_id,subject_id,space_id,source_hash,target,source_key,identity_key,parts,read_requirements,accepted_commit_seq,retired_commit_seq,origin) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(record.namespace_id.as_str()).bind(record.subject_id.as_str()).bind(&record.space_id).bind(&record.source_hash).bind(serde_json::to_string(&record.target)?).bind(record.source_key()?).bind(record.target.identity_key(&record.source_hash)?).bind(serde_json::to_string(&input.parts)?).bind(serde_json::to_string(&input.read_requirements)?).bind(self.commit_seq()).bind(retired.then_some(self.commit_seq())).bind(serde_json::to_string(&value.acceptance)?).execute(&mut *self.tx).await.map_err(write_error)?;
        let sequence = self.commit_seq();
        super::records::insert_record(&mut self.tx, record, sequence).await
    }
}
