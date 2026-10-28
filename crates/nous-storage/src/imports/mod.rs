//! Immutable import container provenance has identity independent of local execution.
use crate::{ReadView, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
mod output;
mod units;

impl WriteTransaction {
    pub async fn insert_import_input_source(&mut self, source: &ImportInputSource) -> Result<()> {
        self.check_sequence(source.commit_seq)?;
        source.id.validate()?;
        self.context()
            .require_subject(&source.namespace_id, &source.subject_id)?;
        self.subject(&source.namespace_id, &source.subject_id)
            .await?;
        let material = self
            .material(&source.namespace_id, &source.material_id)
            .await?;
        if material.content_hash != source.content_hash
            || u64::try_from(material.byte_len).ok() != Some(source.byte_len)
        {
            return Err(Error::invalid(
                "import input source differs from its actual admitted bytes",
            ));
        }
        sqlx::query("INSERT INTO import_input_sources(id,body) VALUES(?,?)")
            .bind(source.id.as_str())
            .bind(serde_json::to_string(source)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn import_input_source(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        id: &ImportInputSourceId,
    ) -> Result<ImportInputSource> {
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM import_input_sources WHERE namespace_id=? AND subject_id=? AND id=?",
        )
        .bind(namespace.as_str())
        .bind(subject.as_str())
        .bind(id.as_str())
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        Ok(serde_json::from_str(&body.ok_or_else(not_found)?)?)
    }
    pub async fn insert_import_record_container(
        &mut self,
        container: &ImportRecordContainer,
    ) -> Result<()> {
        self.check_sequence(container.commit_seq)?;
        container.id.validate()?;
        self.context()
            .require_subject(&container.namespace_id, &container.subject_id)?;
        self.import_input_source(
            &container.namespace_id,
            &container.subject_id,
            &container.input_source_id,
        )
        .await?;
        let observation = self
            .observation(&container.namespace_id, &container.observation_id)
            .await?;
        let memory_matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_revisions WHERE namespace_id=?1 AND subject_id=?2 AND memory_id=?3 AND material_id=?4 AND observation_id=?5 UNION ALL SELECT 1 FROM memory_purges WHERE namespace_id=?1 AND subject_id=?2 AND memory_id=?3)")
            .bind(container.namespace_id.as_str()).bind(container.subject_id.as_str()).bind(container.memory_id.as_str())
            .bind(container.record_material_id.as_str()).bind(container.observation_id.as_str()).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
        if !memory_matches
            || observation.subject_id != container.subject_id
            || observation.material_id != container.record_material_id
            || observation.kind != ObservationKind::Import
        {
            return Err(Error::invalid(
                "import container differs from its actual admitted record",
            ));
        }
        sqlx::query("INSERT INTO import_record_containers(id,body) VALUES(?,?)")
            .bind(container.id.as_str())
            .bind(serde_json::to_string(container)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
}
impl ReadView {
    pub async fn import_source_facts(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<(Vec<ImportInputSource>, Vec<ImportRecordContainer>)> {
        let inputs:Vec<String>=sqlx::query_scalar("SELECT body FROM import_input_sources WHERE namespace_id=? AND subject_id=? AND commit_seq<=? ORDER BY commit_seq,id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(self.watermark).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let records:Vec<String>=sqlx::query_scalar("SELECT body FROM import_record_containers WHERE namespace_id=? AND subject_id=? AND commit_seq<=? ORDER BY commit_seq,id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(self.watermark).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        Ok((
            inputs
                .into_iter()
                .map(|s| serde_json::from_str(&s).map_err(Error::from))
                .collect::<Result<_>>()?,
            records
                .into_iter()
                .map(|s| serde_json::from_str(&s).map_err(Error::from))
                .collect::<Result<_>>()?,
        ))
    }
    /// Container descriptions do not add media roots or confer original-body access.
    pub async fn import_containers(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        record: &MaterialId,
    ) -> Result<Vec<ImportContainerSource>> {
        let rows:Vec<(String,String)>=sqlx::query_as("SELECT i.body,r.body FROM import_record_containers r JOIN import_input_sources i ON i.id=r.input_source_id WHERE r.namespace_id=? AND r.subject_id=? AND r.record_material_id=? AND r.commit_seq<=? ORDER BY r.commit_seq,r.id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(record.as_str()).bind(self.watermark).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|(input, container)| {
                Ok(ImportContainerSource {
                    input: serde_json::from_str(&input)?,
                    container: serde_json::from_str(&container)?,
                })
            })
            .collect()
    }
}
