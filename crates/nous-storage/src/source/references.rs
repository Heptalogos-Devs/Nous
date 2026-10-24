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
            .bind(namespace.as_str()).bind(subject.as_str()).bind(&target.id).bind(target.revision).fetch_optional(conn).await.map_err(storage_error)?
    } else {
        sqlx::query_scalar("SELECT commit_seq FROM structured_revisions WHERE namespace_id=? AND subject_id=? AND object_id=? AND revision=? AND kind=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(&target.id).bind(target.revision).bind(kind).fetch_optional(conn).await.map_err(storage_error)?
    };
    sequence.ok_or_else(|| {
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
        EvidenceSource::CognitiveRevision{reference}=>validate_reference(conn,namespace,subject,reference).await,
        EvidenceSource::MemoryRevision{memory_id,revision}=>validate_reference(conn,namespace,subject,&CognitiveRef{kind:CognitiveKind::Memory,id:memory_id.to_string(),revision:*revision}).await,
        EvidenceSource::Material{material_id}=>sqlx::query_scalar("SELECT m.commit_seq FROM materials m WHERE m.namespace_id=? AND m.id=? AND EXISTS(SELECT 1 FROM observations o WHERE o.namespace_id=m.namespace_id AND o.subject_id=? AND o.material_id=m.id)")
            .bind(namespace.as_str()).bind(material_id.as_str()).bind(subject.as_str()).fetch_optional(conn).await.map_err(storage_error)?.ok_or_else(not_found),
        EvidenceSource::Observation{observation_id}=>sqlx::query_scalar("SELECT commit_seq FROM observations WHERE namespace_id=? AND subject_id=? AND id=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(observation_id.as_str()).fetch_optional(conn).await.map_err(storage_error)?.ok_or_else(not_found),
    }
}

pub(crate) fn source_row(row: &sqlx::sqlite::SqliteRow) -> Result<EvidenceSource> {
    use sqlx::Row;
    let kind: &str = row.get("source_kind");
    let id: String = row.get("source_id");
    let revision: Option<i64> = row.get("source_revision");
    Ok(match kind {
        "material" => EvidenceSource::Material {
            material_id: MaterialId(id),
        },
        "observation" => EvidenceSource::Observation {
            observation_id: ObservationId(id),
        },
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
