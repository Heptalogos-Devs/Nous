use crate::{Database, ReadView, WriteTransaction, storage_error, write_error};
use nous_domain::{media::*, *};
use sqlx::{Row, SqliteConnection};

impl Database {
    pub async fn media_preparation(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        source: &MediaReference,
        plan_digest: &str,
    ) -> Result<Option<MediaPreparationRecord>> {
        let mut view = self.read_view().await?;
        let value = view
            .media_preparation(namespace, subject, source, plan_digest)
            .await?;
        view.finish().await?;
        Ok(value)
    }
}
macro_rules! preparation_read {
    ($owner:ty) => {
        impl $owner {
            pub async fn media_preparation(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                source: &MediaReference,
                plan_digest: &str,
            ) -> Result<Option<MediaPreparationRecord>> {
                read(&mut self.tx, namespace, subject, source, plan_digest).await
            }
        }
    };
}
preparation_read!(ReadView);
preparation_read!(WriteTransaction);
impl WriteTransaction {
    pub async fn put_media_preparation(&mut self, value: &MediaPreparationRecord) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        self.subject(&value.namespace_id, &value.subject_id).await?;
        let duration = self
            .media_duration(&value.namespace_id, &value.subject_id, &value.source)
            .await?;
        value.validate(duration)?;
        let roots = self
            .media_roots(&value.namespace_id, &value.subject_id, &value.source)
            .await?;
        let mut prepared_roots = std::collections::BTreeSet::new();
        for item in &value.graph.inputs {
            prepared_roots.extend(
                self.media_roots(&value.namespace_id, &value.subject_id, &item.source)
                    .await?,
            );
            if let Some(reference) = &item.text_source {
                let projection = self
                    .representation(&value.namespace_id, &value.subject_id, reference)
                    .await?;
                if !matches!(projection.payload, RepresentationPayload::Text { .. }) {
                    return Err(Error::invalid(
                        "prepared text source must identify stored text",
                    ));
                }
                prepared_roots.extend(
                    self.media_roots(
                        &value.namespace_id,
                        &value.subject_id,
                        &MediaReference::Representation {
                            reference: reference.clone(),
                        },
                    )
                    .await?,
                );
            }
        }
        if roots != prepared_roots {
            return Err(Error::invalid(
                "prepared media mapping changes its original source closure",
            ));
        }
        if let Some(existing) = self
            .media_preparation(
                &value.namespace_id,
                &value.subject_id,
                &value.source,
                &value.plan_digest,
            )
            .await?
        {
            if serde_json::to_value(existing.graph)? != serde_json::to_value(&value.graph)? {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "prepared media mapping is immutable",
                ));
            }
            return Ok(());
        }
        sqlx::query("INSERT INTO media_preparations(namespace_id,subject_id,source,plan_digest,graph,recorded_at,commit_seq) VALUES(?,?,?,?,?,?,?)")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(serde_json::to_string(&value.source)?).bind(&value.plan_digest)
            .bind(serde_json::to_string(&value.graph)?).bind(&value.recorded_at).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
async fn read(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    source: &MediaReference,
    plan_digest: &str,
) -> Result<Option<MediaPreparationRecord>> {
    let row = sqlx::query("SELECT graph,recorded_at,commit_seq FROM media_preparations WHERE namespace_id=? AND subject_id=? AND source=? AND plan_digest=?")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(serde_json::to_string(source)?).bind(plan_digest).fetch_optional(conn).await.map_err(storage_error)?;
    row.map(|row| {
        Ok(MediaPreparationRecord {
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            source: source.clone(),
            plan_digest: plan_digest.into(),
            graph: serde_json::from_str(row.get("graph"))?,
            recorded_at: row.get("recorded_at"),
            commit_seq: row.get("commit_seq"),
        })
    })
    .transpose()
}
