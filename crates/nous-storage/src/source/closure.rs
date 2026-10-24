use crate::source::references::*;
use crate::storage_error;
use nous_domain::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Available,
    Retired,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceNode {
    pub source: EvidenceSource,
    pub status: SourceStatus,
    pub commit_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRoot {
    pub material_id: MaterialId,
    pub status: SourceStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceClosure {
    pub reference: CognitiveRef,
    pub nodes: Vec<SourceNode>,
    pub roots: Vec<SourceRoot>,
    pub complete: bool,
}

pub(crate) async fn closure(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: CognitiveRef,
) -> Result<SourceClosure> {
    closure_at(conn, namespace, subject, reference, None).await
}

pub(crate) async fn closure_at(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: CognitiveRef,
    as_of_commit: Option<i64>,
) -> Result<SourceClosure> {
    let (nodes, roots) = resolve_sources_at(
        conn,
        namespace,
        subject,
        vec![source_reference(&reference)],
        false,
        as_of_commit,
    )
    .await?;
    Ok(SourceClosure {
        reference,
        complete: nodes.iter().all(|n| n.status != SourceStatus::Missing),
        nodes,
        roots,
    })
}

pub(crate) async fn resolve_sources(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    sources: Vec<EvidenceSource>,
    support_only: bool,
) -> Result<(Vec<SourceNode>, Vec<SourceRoot>)> {
    resolve_sources_at(conn, namespace, subject, sources, support_only, None).await
}

pub(crate) async fn resolve_sources_at(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    sources: Vec<EvidenceSource>,
    support_only: bool,
    as_of_commit: Option<i64>,
) -> Result<(Vec<SourceNode>, Vec<SourceRoot>)> {
    let mut stack = sources
        .into_iter()
        .rev()
        .map(|source| (source, false, 0usize))
        .collect::<Vec<_>>();
    let mut active = HashSet::new();
    let mut visited = HashSet::new();
    let mut nodes = BTreeMap::new();
    let mut roots = BTreeMap::new();
    while let Some((source, exit, depth)) = stack.pop() {
        let (kind, id, revision) = source_parts(&source)?;
        let identity = format!("{kind}:{id}");
        let key = format!("{identity}@{}", revision.unwrap_or(0));
        if exit {
            active.remove(&key);
            visited.insert(key);
            continue;
        }
        if active.contains(&key) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "source closure contains a cognition cycle",
            ));
        }
        if visited.contains(&key) {
            continue;
        }
        if depth > 64 || nodes.len() >= 1024 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "source closure exceeds 64 levels or 1024 nodes",
            ));
        }
        active.insert(key.clone());
        stack.push((source.clone(), true, depth));
        let (status, commit_seq, children) = source_node(
            conn,
            namespace,
            subject,
            &source,
            support_only,
            as_of_commit,
        )
        .await?;
        if let EvidenceSource::Material { material_id } = &source {
            roots.insert(
                material_id.to_string(),
                SourceRoot {
                    material_id: material_id.clone(),
                    status,
                },
            );
        }
        nodes.insert(
            key,
            SourceNode {
                source,
                status,
                commit_seq,
            },
        );
        for child in children.into_iter().rev() {
            stack.push((child, false, depth + 1));
        }
    }
    Ok((nodes.into_values().collect(), roots.into_values().collect()))
}

pub(crate) async fn source_node(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    source: &EvidenceSource,
    support_only: bool,
    as_of_commit: Option<i64>,
) -> Result<(SourceStatus, Option<i64>, Vec<EvidenceSource>)> {
    let (kind, id, revision) = source_parts(source)?;
    if kind == "material" {
        let sequence = validate_source(conn, namespace, subject, source).await;
        return match sequence {
            Ok(seq) if as_of_commit.is_none_or(|upper| seq <= upper) => {
                Ok((SourceStatus::Available, Some(seq), vec![]))
            }
            Ok(_) => Ok((SourceStatus::Missing, None, vec![])),
            Err(error) if error.code == ErrorCode::NotFound => {
                Ok((SourceStatus::Missing, None, vec![]))
            }
            Err(error) => Err(error),
        };
    }
    if kind == "observation" {
        let row=sqlx::query("SELECT material_id,commit_seq FROM observations WHERE namespace_id=? AND subject_id=? AND id=? AND (? IS NULL OR commit_seq<=?)")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(id).bind(as_of_commit).bind(as_of_commit).fetch_optional(conn).await.map_err(storage_error)?;
        return Ok(if let Some(row) = row {
            (
                SourceStatus::Available,
                Some(row.get("commit_seq")),
                vec![EvidenceSource::Material {
                    material_id: MaterialId(row.get("material_id")),
                }],
            )
        } else {
            (SourceStatus::Missing, None, vec![])
        });
    }
    if kind == "memory" {
        let row=sqlx::query("SELECT r.observation_id,r.commit_seq,r.deleted OR c.deleted AS retired FROM memory_revisions r JOIN memories m ON m.id=r.memory_id JOIN memory_revisions c ON c.memory_id=m.id AND c.revision=(SELECT max(h.revision) FROM memory_revisions h WHERE h.memory_id=m.id AND (? IS NULL OR h.commit_seq<=?)) WHERE r.namespace_id=? AND r.subject_id=? AND r.memory_id=? AND r.revision=? AND (? IS NULL OR r.commit_seq<=?)")
            .bind(as_of_commit).bind(as_of_commit).bind(namespace.as_str()).bind(subject.as_str()).bind(id).bind(revision).bind(as_of_commit).bind(as_of_commit).fetch_optional(conn).await.map_err(storage_error)?;
        return Ok(if let Some(row) = row {
            (
                if row.get::<bool, _>("retired") {
                    SourceStatus::Retired
                } else {
                    SourceStatus::Available
                },
                Some(row.get("commit_seq")),
                vec![EvidenceSource::Observation {
                    observation_id: nous_domain::ObservationId(row.get("observation_id")),
                }],
            )
        } else {
            (SourceStatus::Missing, None, vec![])
        });
    }
    let row=sqlx::query("SELECT r.commit_seq,r.deleted OR c.deleted AS retired FROM structured_revisions r JOIN structured_objects o ON o.id=r.object_id JOIN structured_revisions c ON c.object_id=o.id AND c.revision=(SELECT max(h.revision) FROM structured_revisions h WHERE h.object_id=o.id AND (? IS NULL OR h.commit_seq<=?)) WHERE r.namespace_id=? AND r.subject_id=? AND r.object_id=? AND r.revision=? AND r.kind=? AND (? IS NULL OR r.commit_seq<=?)")
        .bind(as_of_commit).bind(as_of_commit).bind(namespace.as_str()).bind(subject.as_str()).bind(&id).bind(revision).bind(kind).bind(as_of_commit).bind(as_of_commit).fetch_optional(&mut *conn).await.map_err(storage_error)?;
    let Some(row) = row else {
        return Ok((SourceStatus::Missing, None, vec![]));
    };
    let children=sqlx::query("SELECT * FROM structured_links WHERE object_id=? AND revision=? AND (?=0 OR polarity='support') ORDER BY category,ordinal").bind(id).bind(revision).bind(support_only).fetch_all(conn).await.map_err(storage_error)?.iter().map(source_row).collect::<Result<Vec<_>>>()?;
    Ok((
        if row.get::<bool, _>("retired") {
            SourceStatus::Retired
        } else {
            SourceStatus::Available
        },
        Some(row.get("commit_seq")),
        children,
    ))
}
