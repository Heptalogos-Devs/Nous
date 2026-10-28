use crate::codec::*;
use crate::{WriteTransaction, not_found, storage_error};
use nous_domain::*;
use sqlx::SqliteConnection;

impl WriteTransaction {
    pub async fn validate_reference(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        validate_reference(&mut self.tx, namespace, subject, reference)
            .await
            .map(|_| ())
    }
}

pub(crate) fn reference(value: &StructuredObject) -> CognitiveRef {
    CognitiveRef {
        kind: value.kind(),
        id: value.id.to_string(),
        revision: value.revision,
    }
}

pub(crate) fn source_reference(target: &CognitiveRef) -> EvidenceSource {
    if target.kind == CognitiveKind::Memory {
        EvidenceSource::MemoryRevision {
            memory_id: nous_domain::MemoryId(target.id.clone()),
            revision: target.revision,
        }
    } else {
        EvidenceSource::CognitiveRevision {
            reference: target.clone(),
        }
    }
}

pub(crate) fn source_parts(source: &EvidenceSource) -> Result<(String, String, Option<i64>)> {
    Ok(match source {
        EvidenceSource::Material { material_id } => {
            ("material".into(), material_id.to_string(), None)
        }
        EvidenceSource::Media { reference } => (
            "media".into(),
            RetrievalTarget::Media(reference.clone()).key()?,
            None,
        ),
        EvidenceSource::Observation { observation_id } => {
            ("observation".into(), observation_id.to_string(), None)
        }
        EvidenceSource::MemoryRevision {
            memory_id,
            revision,
        } => ("memory".into(), memory_id.to_string(), Some(*revision)),
        EvidenceSource::CognitiveRevision { reference } => (
            enum_name(&reference.kind)?,
            reference.id.clone(),
            Some(reference.revision),
        ),
    })
}

pub(crate) async fn validate_reference(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    target: &CognitiveRef,
) -> Result<i64> {
    if target.revision <= 0 {
        return Err(Error::invalid(
            "cognitive reference must name a positive exact revision",
        ));
    }
    let kind = enum_name(&target.kind)?;
    let sequence: Option<i64> = if kind == "memory" {
        sqlx::query_scalar("SELECT commit_seq FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(&target.id).bind(target.revision).fetch_optional(&mut *conn).await.map_err(storage_error)?
    } else {
        sqlx::query_scalar("SELECT commit_seq FROM structured_revisions WHERE namespace_id=? AND subject_id=? AND object_id=? AND revision=? AND kind=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(&target.id).bind(target.revision).bind(kind).fetch_optional(&mut *conn).await.map_err(storage_error)?
    };
    if let Some(sequence) = sequence {
        return Ok(sequence);
    }
    if let Some(tombstone) = super::tombstone::read(
        conn,
        namespace,
        subject,
        &RetrievalTarget::Cognition(target.clone()),
    )
    .await?
    {
        return Ok(tombstone.commit_seq);
    }
    if let Some(sequence) =
        super::tombstone::purged_sequence(conn, namespace, subject, target).await?
    {
        return Ok(sequence);
    }
    Err({
        Error::new(
            ErrorCode::NotFound,
            "exact cognition reference is unavailable in this scope",
        )
    })
}

pub(crate) async fn validate_source(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    source: &EvidenceSource,
) -> Result<i64> {
    match source {
        EvidenceSource::CognitiveRevision { reference } => {
            validate_reference(conn, namespace, subject, reference).await
        }
        EvidenceSource::MemoryRevision {
            memory_id,
            revision,
        } => {
            validate_reference(
                conn,
                namespace,
                subject,
                &CognitiveRef {
                    kind: CognitiveKind::Memory,
                    id: memory_id.to_string(),
                    revision: *revision,
                },
            )
            .await
        }
        EvidenceSource::Material { material_id } => {
            material_source(conn, namespace, subject, material_id).await
        }
        EvidenceSource::Media { reference } => media_source(conn, namespace, subject, reference)
            .await?
            .commit_seq(),
        EvidenceSource::Observation { observation_id } => sqlx::query_scalar(
            "SELECT commit_seq FROM observations WHERE namespace_id=? AND subject_id=? AND id=?",
        )
        .bind(namespace.as_str())
        .bind(subject.as_str())
        .bind(observation_id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found),
    }
}

async fn material_source(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    id: &MaterialId,
) -> Result<i64> {
    Ok(
        crate::source::materials::require_subject_material(conn, namespace, subject, id)
            .await?
            .commit_seq,
    )
}
pub(crate) async fn media_source(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &media::MediaReference,
) -> Result<media::MediaSourceLineage> {
    let lineage = crate::media::lineage_in(conn, namespace, subject, reference, 0).await?;
    for root in &lineage.materials {
        material_source(conn, namespace, subject, &root.material.id).await?;
    }
    Ok(lineage)
}

pub(crate) fn source_row(row: &sqlx::sqlite::SqliteRow) -> Result<EvidenceSource> {
    use sqlx::Row;
    let kind: &str = row.get("source_kind");
    let id: String = row.get("source_id");
    let revision: Option<i64> = row.get("source_revision");
    source_from_parts(kind, id, revision)
}

pub(crate) fn source_from_parts(
    kind: &str,
    id: String,
    revision: Option<i64>,
) -> Result<EvidenceSource> {
    Ok(match kind {
        "material" => EvidenceSource::Material {
            material_id: MaterialId(id),
        },
        "observation" => EvidenceSource::Observation {
            observation_id: ObservationId(id),
        },
        "media" => {
            let target: RetrievalTarget = id.parse()?;
            let RetrievalTarget::Media(reference) = target else {
                return Err(Error::internal(
                    "persisted media evidence key names a cognition",
                ));
            };
            EvidenceSource::Media { reference }
        }
        "memory" => EvidenceSource::MemoryRevision {
            memory_id: MemoryId(id),
            revision: revision.ok_or_else(|| Error::internal("missing exact source revision"))?,
        },
        _ => EvidenceSource::CognitiveRevision {
            reference: CognitiveRef {
                kind: crate::codec::parse_enum(kind)?,
                id,
                revision: revision
                    .ok_or_else(|| Error::internal("missing exact source revision"))?,
            },
        },
    })
}
