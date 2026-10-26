use super::references::{source_from_parts, validate_source};
use crate::StructuredFormationRun;
use crate::codec::*;
use crate::{WriteTransaction, storage_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection};

impl WriteTransaction {
    /// Validate captured source identities under the enqueue writer lock. A
    /// source read before this transaction cannot race a permanent purge.
    pub(crate) async fn validate_structured_formation_sources(
        &mut self,
        value: &StructuredFormationRun,
    ) -> Result<()> {
        let (namespace, subject) = value.job.subject_scope()?;
        let Some(sources) = value.input.get("sources") else {
            return Ok(());
        };
        let sources = sources
            .as_array()
            .ok_or_else(|| Error::invalid("frozen formation sources must be an array"))?;
        for entry in sources {
            let source: EvidenceSource =
                serde_json::from_value(entry.get("source").unwrap_or(entry).clone())?;
            source.validate()?;
            let dependencies =
                source_dependencies(&mut self.tx, namespace, subject, &source).await?;
            for dependency in dependencies {
                match dependency {
                    EvidenceSource::Media { reference } => {
                        validate_source(
                            &mut self.tx,
                            namespace,
                            subject,
                            &EvidenceSource::Media { reference },
                        )
                        .await?;
                    }
                    EvidenceSource::Material { material_id } => {
                        self.material(namespace, &material_id).await?;
                        let exposed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations WHERE namespace_id=? AND subject_id=? AND material_id=?)")
                            .bind(namespace.as_str()).bind(subject.as_str()).bind(material_id.as_str()).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
                        if !exposed {
                            return Err(Error::new(
                                ErrorCode::NotFound,
                                "formation material is unavailable in this subject",
                            ));
                        }
                    }
                    EvidenceSource::Observation { observation_id } => {
                        if self
                            .observation(namespace, &observation_id)
                            .await?
                            .subject_id
                            != *subject
                        {
                            return Err(Error::new(
                                ErrorCode::NotFound,
                                "formation observation is unavailable in this subject",
                            ));
                        }
                    }
                    EvidenceSource::MemoryRevision {
                        memory_id,
                        revision,
                    } => {
                        self.validate_reference(
                            namespace,
                            subject,
                            &CognitiveRef::memory(&memory_id, revision),
                        )
                        .await?;
                        let exact = crate::vector::source::memory(
                            &mut self.tx,
                            namespace,
                            subject,
                            &memory_id,
                            revision,
                        )
                        .await?
                        .ok_or_else(crate::not_found)?;
                        self.validate_memory_source(
                            namespace,
                            subject,
                            &exact.observation_id,
                            &exact.material_id,
                            &exact.source,
                        )
                        .await?;
                        let current = self.memory(namespace, &memory_id).await?;
                        let deleted: bool = sqlx::query_scalar(
                            "SELECT deleted FROM memory_revisions WHERE memory_id=? AND revision=?",
                        )
                        .bind(memory_id.as_str())
                        .bind(revision)
                        .fetch_one(&mut *self.tx)
                        .await
                        .map_err(storage_error)?;
                        if current.deleted || deleted {
                            return Err(Error::new(
                                ErrorCode::Conflict,
                                "formation Memory source is retired",
                            ));
                        }
                    }
                    EvidenceSource::CognitiveRevision { reference } => {
                        self.validate_reference(namespace, subject, &reference)
                            .await?;
                        let id = CognitiveId(reference.id);
                        if self
                            .structured_revision(namespace, &id, reference.revision)
                            .await?
                            .deleted
                            || self.structured_object(namespace, &id).await?.deleted
                        {
                            return Err(Error::new(
                                ErrorCode::Conflict,
                                "formation cognition source is retired",
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

async fn source_dependencies(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    source: &EvidenceSource,
) -> Result<Vec<EvidenceSource>> {
    let Some(reference) = source.cognitive_ref() else {
        return Ok(vec![source.clone()]);
    };
    let kind = enum_name(&reference.kind)?;
    let rows=sqlx::query("WITH RECURSIVE walk(kind,id,revision) AS (SELECT ?,?,? UNION SELECT l.source_kind,l.source_id,l.source_revision FROM walk w JOIN structured_revisions r ON r.kind=w.kind AND r.object_id=w.id AND r.revision=w.revision JOIN structured_links l ON l.object_id=r.object_id AND l.revision=r.revision WHERE l.namespace_id=? AND l.subject_id=?) SELECT kind,id,revision FROM walk LIMIT 1025")
        .bind(kind).bind(reference.id).bind(reference.revision).bind(namespace.as_str()).bind(subject.as_str()).fetch_all(conn).await.map_err(storage_error)?;
    if rows.len() > 1024 {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "formation source closure exceeds 1024 references",
        ));
    }
    rows.into_iter()
        .map(|row| source_from_parts(row.get("kind"), row.get("id"), row.get("revision")))
        .collect()
}
