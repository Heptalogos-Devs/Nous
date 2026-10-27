//! Original source reads and visibility checks share the authority snapshot.
use crate::{
    Database, ReadView, WriteTransaction, codec::parse_optional, not_found, storage_error,
    write_error,
};
use nous_domain::{Material, MaterialDetails, MaterialId, NamespaceId, Result, SubjectId};
use sqlx::{Row, SqliteConnection};

impl ReadView {
    pub async fn material_by_id(&mut self, id: &MaterialId) -> Result<Material> {
        sqlx::query("SELECT * FROM materials WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&mut *self.tx)
            .await
            .map_err(storage_error)?
            .map(crate::authority::records::material_row)
            .transpose()?
            .ok_or_else(not_found)
    }
    pub async fn subject_has_material_access(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        material: &MaterialId,
    ) -> Result<bool> {
        material_access(&mut self.tx, namespace, subject, material).await
    }
    pub async fn material_details(
        &mut self,
        namespace: &NamespaceId,
        id: &MaterialId,
    ) -> Result<MaterialDetails> {
        material_details(&mut self.tx, namespace, id).await
    }
}

impl Database {
    pub async fn subject_has_material_access(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        material: &MaterialId,
    ) -> Result<bool> {
        material_access(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            subject,
            material,
        )
        .await
    }
    pub async fn material_details(
        &self,
        namespace: &NamespaceId,
        id: &MaterialId,
    ) -> Result<MaterialDetails> {
        material_details(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await
    }
}

async fn material_access(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    material: &MaterialId,
) -> Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations WHERE namespace_id=? AND subject_id=? AND material_id=? UNION ALL SELECT 1 FROM material_outputs WHERE namespace_id=? AND subject_id=? AND material_id=?)")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(material.as_str())
        .bind(namespace.as_str()).bind(subject.as_str()).bind(material.as_str())
        .fetch_one(conn).await.map_err(storage_error)
}

pub(crate) async fn material_details(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &MaterialId,
) -> Result<MaterialDetails> {
    let row=sqlx::query("SELECT d.source_type,d.tags,d.metadata,d.structured FROM materials m LEFT JOIN material_details d ON d.namespace_id=m.namespace_id AND d.material_id=m.id WHERE m.namespace_id=? AND m.id=?")
        .bind(namespace.as_str()).bind(id.as_str()).fetch_optional(conn).await.map_err(storage_error)?.ok_or_else(not_found)?;
    details_from_columns(
        row.get("source_type"),
        row.get("tags"),
        row.get("metadata"),
        row.get("structured"),
    )
}

pub(crate) fn details_from_columns(
    source_type: Option<String>,
    tags: Option<&str>,
    metadata: Option<&str>,
    structured: Option<&str>,
) -> Result<MaterialDetails> {
    Ok(MaterialDetails {
        source_type,
        tags: serde_json::from_str(tags.unwrap_or("[]")).map_err(storage_error)?,
        metadata: serde_json::from_str(metadata.unwrap_or("{}")).map_err(storage_error)?,
        structured: parse_optional(structured)?,
    })
}

impl WriteTransaction {
    /// Retain a newly produced resource for its owner without asserting that the
    /// Subject has read its contents. Existing source access still needs authority.
    pub async fn retain_material_output(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        material: &MaterialId,
    ) -> Result<()> {
        self.context().require_subject(namespace, subject)?;
        self.subject(namespace, subject).await?;
        let original = self.material(namespace, material).await?;
        self.check_sequence(original.commit_seq)?;
        sqlx::query("INSERT INTO material_outputs(namespace_id,subject_id,material_id,context,recorded_at,commit_seq) VALUES(?,?,?,?,?,?)")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(material.as_str())
            .bind(serde_json::to_string(self.context())?).bind(self.time().recorded())
            .bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn subject_has_material_access(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        material: &MaterialId,
    ) -> Result<bool> {
        material_access(&mut self.tx, namespace, subject, material).await
    }

    pub async fn set_material_details(
        &mut self,
        namespace: &NamespaceId,
        id: &MaterialId,
        details: &MaterialDetails,
    ) -> Result<()> {
        self.material(namespace, id).await?;
        sqlx::query("INSERT INTO material_details(material_id,namespace_id,source_type,tags,metadata,structured) VALUES (?,?,?,?,?,?) ON CONFLICT(material_id) DO UPDATE SET source_type=excluded.source_type,tags=excluded.tags,metadata=excluded.metadata,structured=excluded.structured")
            .bind(id.as_str()).bind(namespace.as_str()).bind(&details.source_type).bind(serde_json::to_string(&details.tags).map_err(storage_error)?)
            .bind(serde_json::to_string(&details.metadata).map_err(storage_error)?).bind(details.structured.as_ref().map(serde_json::to_string).transpose().map_err(storage_error)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
