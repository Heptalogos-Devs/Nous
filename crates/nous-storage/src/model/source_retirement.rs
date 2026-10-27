use crate::{WriteTransaction, storage_error};
use nous_domain::*;
use serde_json::Value;
use sqlx::Row;
use std::collections::BTreeSet;

impl WriteTransaction {
    /// Map actual input sources before erasure; imported ledger job identities
    /// never become local task identities.
    pub async fn retire_model_source_payloads(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        targets: &[RetrievalTarget],
    ) -> Result<Vec<String>> {
        let targets = targets.iter().cloned().collect::<BTreeSet<_>>();
        let rows=sqlx::query("SELECT id,local_job_id FROM model_calls WHERE namespace_id=? AND subject_id=? AND json_extract(request,'$.preview.input.kind') IN ('chat','rerank','embedding')")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut jobs = BTreeSet::new();
        for row in rows {
            let call = self.model_call(&ModelCallId(row.get("id"))).await?;
            if call
                .request
                .pointer("/preview/input/kind")
                .and_then(Value::as_str)
                == Some("embedding")
            {
                self.redact_embedding_sources(&call, &targets).await?;
                continue;
            }
            let Some(sources) = call
                .request
                .pointer("/preview/sources")
                .filter(|value| !value.is_null())
            else {
                continue;
            };
            let sources: Vec<SourceReadRef> = serde_json::from_value(sources.clone())?;
            let mapped = self.imported_model_sources(&call, &sources).await?;
            if mapped
                .iter()
                .filter_map(SourceReadRef::retrieval_target)
                .any(|source| targets.contains(&source))
            {
                self.redact_call_payload(&call.id, "source_permanently_purged")
                    .await?;
                if let Some(id) = row.get::<Option<String>, _>("local_job_id") {
                    jobs.insert(id);
                }
            }
        }
        Ok(jobs.into_iter().collect())
    }
}
