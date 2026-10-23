//! Exact cognition revisions and source closure in the caller's authority view.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous persistence/src/references.rs and memory/src/service/
// provenance.rs, episode/mutation.rs, journal/mutations.rs and schema/mutation.rs,
// commit 412b062082afb75ccfae4c3f69fc606d3e3cc862. See LICENSE.nous and NOTICE.
// SQLite shares the operation transaction, scoped revision owner and exact edges.
use std::collections::{BTreeMap, HashSet};

use nous_domain::{
    CognitiveContent, CognitiveId, CognitiveKind, CognitiveRef, Error, ErrorCode, EvidenceLink,
    EvidenceSource, MaterialId, NamespaceId, Result, StructuredObject, SubjectId, TimeRange,
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use crate::{Database, WriteTransaction, not_found, storage_error, write_error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSnapshot {
    pub watermark: i64,
    pub objects: Vec<StructuredObject>,
    pub complete: bool,
}

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

impl Database {
    pub async fn structured_namespace(&self, id: &CognitiveId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM structured_objects WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn structured_object(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<StructuredObject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let object = object(&mut tx, namespace, id, None).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(object)
    }

    pub async fn structured_revision(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
        revision: i64,
    ) -> Result<StructuredObject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let object = object(&mut tx, namespace, id, Some(revision)).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(object)
    }

    pub async fn structured_history(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<Vec<StructuredObject>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let revisions: Vec<i64> = sqlx::query_scalar("SELECT revision FROM structured_revisions WHERE namespace_id=? AND object_id=? ORDER BY revision")
            .bind(namespace.as_str()).bind(id.as_str()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if revisions.is_empty() {
            return Err(not_found());
        }
        let mut history = Vec::with_capacity(revisions.len());
        for revision in revisions {
            history.push(object(&mut tx, namespace, id, Some(revision)).await?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(history)
    }

    pub async fn subject_structured_snapshot(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        kind: Option<CognitiveKind>,
        as_of_commit: Option<i64>,
        limit: usize,
    ) -> Result<StructuredSnapshot> {
        let fetch_limit = i64::try_from(limit)
            .ok()
            .filter(|v| *v > 0 && *v <= 10000)
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| Error::invalid("structured snapshot limit must be 1..10000"))?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current: i64 =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let watermark = as_of_commit.unwrap_or(current);
        if watermark < 0 || watermark > current {
            return Err(Error::invalid("snapshot refers to uncommitted authority"));
        }
        let kind = kind.as_ref().map(enum_name).transpose()?;
        let rows=sqlx::query("SELECT r.* FROM structured_revisions r WHERE r.namespace_id=? AND r.subject_id=? AND (? IS NULL OR r.kind=?) AND r.revision=(SELECT max(v.revision) FROM structured_revisions v WHERE v.object_id=r.object_id AND v.commit_seq<=?) ORDER BY r.object_id LIMIT ?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(&kind).bind(&kind).bind(watermark).bind(fetch_limit).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let complete = rows.len() <= limit;
        let mut objects = Vec::with_capacity(rows.len().min(limit));
        for row in rows.into_iter().take(limit) {
            objects.push(decode(&mut tx, row).await?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(StructuredSnapshot {
            watermark,
            objects,
            complete,
        })
    }

    pub async fn source_closure(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
        revision: Option<i64>,
    ) -> Result<SourceClosure> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let owner = object(&mut tx, namespace, id, revision).await?;
        let reference = reference(&owner);
        let result = closure(&mut tx, namespace, &owner.subject_id, reference).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }

    pub async fn concept_by_alias(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        alias: &str,
    ) -> Result<StructuredObject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let id:String=sqlx::query_scalar("SELECT concept_id FROM concept_aliases WHERE namespace_id=? AND subject_id=? AND alias_key=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(alias.trim().to_lowercase()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or_else(not_found)?;
        let result = object(&mut tx, namespace, &CognitiveId(id), None).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}

impl WriteTransaction {
    pub async fn structured_object(
        &mut self,
        namespace: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<StructuredObject> {
        object(&mut self.tx, namespace, id, None).await
    }
    pub async fn structured_revision(
        &mut self,
        namespace: &NamespaceId,
        id: &CognitiveId,
        revision: i64,
    ) -> Result<StructuredObject> {
        object(&mut self.tx, namespace, id, Some(revision)).await
    }
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

    /// One revision owner, exact typed inputs, and a scoped compare-and-swap.
    pub async fn save_structured(
        &mut self,
        value: &StructuredObject,
        expected_revision: Option<i64>,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        value.validate()?;
        let revision = expected_revision
            .unwrap_or(0)
            .checked_add(1)
            .filter(|v| *v > 0)
            .ok_or_else(|| Error::invalid("structured revision is out of range"))?;
        if value.revision != revision {
            return Err(Error::invalid(
                "structured revision must follow expected_revision",
            ));
        }
        self.subject(&value.namespace_id, &value.subject_id).await?;
        let previous = if let Some(expected) = expected_revision {
            let old = self
                .structured_object(&value.namespace_id, &value.id)
                .await?;
            if old.revision != expected
                || old.subject_id != value.subject_id
                || old.kind() != value.kind()
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "structured identity, scope or revision changed",
                ));
            }
            Some(old)
        } else {
            None
        };
        let mut body = value.clone();
        body.evidence.clear();
        let body_references = body.references();
        let previous_sources = previous
            .as_ref()
            .map(|old| {
                old.sources()
                    .into_iter()
                    .map(|source| source_parts(&source))
                    .collect::<Result<HashSet<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        for target in value.references() {
            if let Err(error) = validate_reference(
                &mut self.tx,
                &value.namespace_id,
                &value.subject_id,
                &target,
            )
            .await
            {
                let retained =
                    previous_sources.contains(&source_parts(&source_reference(&target))?);
                if error.code != ErrorCode::NotFound || !retained {
                    return Err(error);
                }
            }
        }
        let body_evidence = body.body.evidence_links();
        for source in value.sources() {
            if let Err(error) = validate_source(
                &mut self.tx,
                &value.namespace_id,
                &value.subject_id,
                &source,
            )
            .await
            {
                let retained = previous_sources.contains(&source_parts(&source)?);
                if error.code != ErrorCode::NotFound || !retained {
                    return Err(error);
                }
            }
        }
        reject_cycle(&mut self.tx, value).await?;
        validate_content_in(&mut self.tx, value, previous.as_ref()).await?;
        if let Some(call_id) = &value.producer_call_id {
            let call = self.model_call(call_id).await?;
            if call.namespace_id != value.namespace_id
                || call.subject_id != value.subject_id
                || call.state != nous_domain::ModelCallState::Succeeded
            {
                return Err(Error::invalid(
                    "structured producer requires a successful scoped model call",
                ));
            }
        }
        let kind = enum_name(&value.kind())?;
        match expected_revision {
            None => {
                sqlx::query("INSERT INTO structured_objects(id,namespace_id,subject_id,kind,current_revision,commit_seq) VALUES (?,?,?,?,?,?)")
                .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&kind).bind(value.revision).bind(value.commit_seq)
                .execute(&mut *self.tx).await.map_err(write_error)?;
            }
            Some(expected) => {
                let changed=sqlx::query("UPDATE structured_objects SET current_revision=?,commit_seq=? WHERE id=? AND namespace_id=? AND subject_id=? AND kind=? AND current_revision=?")
                .bind(value.revision).bind(value.commit_seq).bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&kind).bind(expected)
                .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
                if changed != 1 {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "structured revision changed",
                    ));
                }
            }
        }
        sqlx::query("INSERT INTO structured_revisions(object_id,namespace_id,subject_id,revision,kind,body,times,producer_call_id,recorded_at,deleted,commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.revision).bind(&kind).bind(serde_json::to_string(&value.body)?)
            .bind(serde_json::to_string(&value.times)?).bind(value.producer_call_id.as_ref().map(|id|id.as_str())).bind(&value.recorded_at).bind(value.deleted).bind(value.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        for (ordinal, target) in body_references.iter().enumerate() {
            let source = source_reference(target);
            save_link(&mut self.tx, value, ordinal, "body", &source, None).await?;
        }
        for (ordinal, link) in value.evidence.iter().enumerate() {
            save_link(
                &mut self.tx,
                value,
                ordinal,
                "evidence",
                &link.source,
                Some(link),
            )
            .await?;
        }
        for (ordinal, link) in body_evidence.iter().enumerate() {
            save_link(
                &mut self.tx,
                value,
                ordinal,
                "body_evidence",
                &link.source,
                Some(link),
            )
            .await?;
        }
        sync_aliases(&mut self.tx, value).await?;
        Ok(())
    }
}

fn reference(value: &StructuredObject) -> CognitiveRef {
    CognitiveRef {
        kind: value.kind(),
        id: value.id.to_string(),
        revision: value.revision,
    }
}

fn source_reference(target: &CognitiveRef) -> EvidenceSource {
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

async fn object(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &CognitiveId,
    revision: Option<i64>,
) -> Result<StructuredObject> {
    let row=sqlx::query("SELECT r.* FROM structured_revisions r JOIN structured_objects o ON o.id=r.object_id WHERE r.namespace_id=? AND r.object_id=? AND r.revision=coalesce(?,o.current_revision)")
        .bind(namespace.as_str()).bind(id.as_str()).bind(revision).fetch_optional(&mut *conn).await.map_err(storage_error)?.ok_or_else(not_found)?;
    decode(conn, row).await
}

async fn decode(conn: &mut SqliteConnection, row: SqliteRow) -> Result<StructuredObject> {
    let id: String = row.get("object_id");
    let revision: i64 = row.get("revision");
    let links=sqlx::query("SELECT source,polarity,relation,explanation FROM structured_links WHERE object_id=? AND revision=? AND category='evidence' ORDER BY ordinal")
        .bind(&id).bind(revision).fetch_all(conn).await.map_err(storage_error)?;
    let evidence = links
        .into_iter()
        .map(|link| {
            Ok(EvidenceLink {
                source: serde_json::from_str(link.get("source"))?,
                polarity: parse_enum(link.get("polarity"))?,
                relation: link
                    .get::<Option<&str>, _>("relation")
                    .map(parse_enum)
                    .transpose()?,
                explanation: link.get("explanation"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(StructuredObject {
        id: CognitiveId(id),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        revision,
        body: serde_json::from_str(row.get("body"))?,
        evidence,
        times: serde_json::from_str(row.get("times"))?,
        producer_call_id: row
            .get::<Option<String>, _>("producer_call_id")
            .map(nous_domain::ModelCallId),
        recorded_at: row.get("recorded_at"),
        commit_seq: row.get("commit_seq"),
        deleted: row.get("deleted"),
    })
}

async fn save_link(
    conn: &mut SqliteConnection,
    owner: &StructuredObject,
    ordinal: usize,
    category: &str,
    source: &EvidenceSource,
    link: Option<&EvidenceLink>,
) -> Result<()> {
    let (kind, id, revision) = source_parts(source)?;
    sqlx::query("INSERT INTO structured_links(namespace_id,subject_id,object_id,revision,ordinal,category,source_kind,source_id,source_revision,polarity,relation,explanation,source) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(owner.namespace_id.as_str()).bind(owner.subject_id.as_str()).bind(owner.id.as_str()).bind(owner.revision).bind(ordinal as i64).bind(category)
        .bind(kind).bind(id).bind(revision).bind(link.map(|l|enum_name(&l.polarity)).transpose()?).bind(link.and_then(|l|l.relation.as_ref()).map(enum_name).transpose()?)
        .bind(link.map(|l|l.explanation.as_str())).bind(serde_json::to_string(source)?).execute(conn).await.map_err(write_error)?;
    Ok(())
}

fn source_parts(source: &EvidenceSource) -> Result<(String, String, Option<i64>)> {
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

async fn validate_reference(
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

async fn validate_source(
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

/// Historical lineage is an exact-revision DAG; redirects have their own
/// current-identity check. Neither is an ordinary support/member dependency.
fn dependencies(value: &StructuredObject) -> Vec<CognitiveRef> {
    let mut value = value.clone();
    match &mut value.body {
        CognitiveContent::Concept(content) => {
            content.canonical = None;
            content.lineage.clear();
        }
        CognitiveContent::Episode(content) => content.lineage.clear(),
        CognitiveContent::Journal(content) => content.lineage.clear(),
        CognitiveContent::Schema(content) => content.lineage.clear(),
        _ => {}
    }
    value.references()
}

async fn reject_cycle(conn: &mut SqliteConnection, owner: &StructuredObject) -> Result<()> {
    let mut stack = dependencies(owner);
    let mut seen = HashSet::new();
    while let Some(target) = stack.pop() {
        if target.id == owner.id.as_str() && target.kind == owner.kind() {
            return Err(Error::invalid(
                "cognition dependency would create an object cycle",
            ));
        }
        if !seen.insert((enum_name(&target.kind)?, target.id.clone(), target.revision)) {
            continue;
        }
        if seen.len() > 1024 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "cognition dependency closure exceeds 1024 revisions",
            ));
        }
        if target.kind == CognitiveKind::Memory {
            continue;
        }
        let nested = object(
            conn,
            &owner.namespace_id,
            &CognitiveId(target.id),
            Some(target.revision),
        )
        .await?;
        stack.extend(dependencies(&nested));
    }
    Ok(())
}

async fn closure(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: CognitiveRef,
) -> Result<SourceClosure> {
    let (nodes, roots) = resolve_sources(
        conn,
        namespace,
        subject,
        vec![EvidenceSource::CognitiveRevision {
            reference: reference.clone(),
        }],
        false,
    )
    .await?;
    Ok(SourceClosure {
        reference,
        complete: nodes.iter().all(|n| n.status != SourceStatus::Missing),
        nodes,
        roots,
    })
}

async fn resolve_sources(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    sources: Vec<EvidenceSource>,
    support_only: bool,
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
        let (status, commit_seq, children) =
            source_node(conn, namespace, subject, &source, support_only).await?;
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

async fn source_node(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    source: &EvidenceSource,
    support_only: bool,
) -> Result<(SourceStatus, Option<i64>, Vec<EvidenceSource>)> {
    let (kind, id, revision) = source_parts(source)?;
    if kind == "material" {
        let sequence = validate_source(conn, namespace, subject, source).await;
        return match sequence {
            Ok(seq) => Ok((SourceStatus::Available, Some(seq), vec![])),
            Err(error) if error.code == ErrorCode::NotFound => {
                Ok((SourceStatus::Missing, None, vec![]))
            }
            Err(error) => Err(error),
        };
    }
    if kind == "observation" {
        let row=sqlx::query("SELECT material_id,commit_seq FROM observations WHERE namespace_id=? AND subject_id=? AND id=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(id).fetch_optional(conn).await.map_err(storage_error)?;
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
        let row=sqlx::query("SELECT r.observation_id,r.commit_seq,r.deleted OR c.deleted AS retired FROM memory_revisions r JOIN memories m ON m.id=r.memory_id JOIN memory_revisions c ON c.memory_id=m.id AND c.revision=m.current_revision WHERE r.namespace_id=? AND r.subject_id=? AND r.memory_id=? AND r.revision=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(id).bind(revision).fetch_optional(conn).await.map_err(storage_error)?;
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
    let row=sqlx::query("SELECT r.commit_seq,r.deleted OR c.deleted AS retired FROM structured_revisions r JOIN structured_objects o ON o.id=r.object_id JOIN structured_revisions c ON c.object_id=o.id AND c.revision=o.current_revision WHERE r.namespace_id=? AND r.subject_id=? AND r.object_id=? AND r.revision=? AND r.kind=?")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(&id).bind(revision).bind(kind).fetch_optional(&mut *conn).await.map_err(storage_error)?;
    let Some(row) = row else {
        return Ok((SourceStatus::Missing, None, vec![]));
    };
    let children=sqlx::query_scalar::<_,String>("SELECT source FROM structured_links WHERE object_id=? AND revision=? AND (?=0 OR polarity='support') ORDER BY category,ordinal")
        .bind(id).bind(revision).bind(support_only).fetch_all(conn).await.map_err(storage_error)?.into_iter().map(|s|serde_json::from_str(&s).map_err(storage_error)).collect::<Result<Vec<_>>>()?;
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

async fn validate_content_in(
    conn: &mut SqliteConnection,
    value: &StructuredObject,
    _previous: Option<&StructuredObject>,
) -> Result<()> {
    match &value.body {
        CognitiveContent::MemorySemantics(content) => {
            let memory=sqlx::query("SELECT material_id FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=?")
                .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(content.memory_id.as_str()).bind(content.memory_revision)
                .fetch_optional(&mut *conn).await.map_err(storage_error)?;
            if let (Some(memory), Some(grounding)) = (memory, &content.grounding_observation) {
                let needs_contact =
                    content.cognitive_role != nous_domain::CognitiveRole::Declarative;
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations WHERE namespace_id=? AND subject_id=? AND id=? AND material_id=? AND (?=0 OR kind='contact'))")
                    .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(grounding.as_str()).bind(memory.get::<String,_>("material_id"))
                    .bind(needs_contact).fetch_one(&mut *conn).await.map_err(storage_error)?;
                if !valid {
                    return Err(Error::invalid(
                        "Memory semantics grounding must match its source material; experiential roles require Contact",
                    ));
                }
            }
            if content.formation_mode == nous_domain::FormationMode::Synthesized {
                let sources = value
                    .evidence_links()
                    .into_iter()
                    .filter(|link| link.polarity == nous_domain::EvidencePolarity::Support)
                    .map(|link| link.source)
                    .collect();
                require_independent_roots(conn, value, sources).await?;
            }
        }
        CognitiveContent::Concept(content) => {
            let mut target = content.canonical.clone();
            let mut seen = HashSet::from([value.id.to_string()]);
            while let Some(canonical) = target {
                if !seen.insert(canonical.id.clone()) {
                    return Err(Error::invalid(
                        "Concept canonical redirects contain an identity cycle",
                    ));
                }
                if seen.len() > 64 {
                    return Err(Error::new(
                        ErrorCode::ResourceExhausted,
                        "Concept canonical chain exceeds 64 identities",
                    ));
                }
                let current =
                    object(conn, &value.namespace_id, &CognitiveId(canonical.id), None).await?;
                let CognitiveContent::Concept(current) = current.body else {
                    return Err(Error::invalid("canonical target must be a Concept"));
                };
                target = current.canonical;
            }
        }
        CognitiveContent::Episode(content) if !value.deleted => {
            if let Some(parent) = &content.parent {
                let parent = object(
                    conn,
                    &value.namespace_id,
                    &CognitiveId(parent.id.clone()),
                    Some(parent.revision),
                )
                .await?;
                let CognitiveContent::Episode(parent) = parent.body else {
                    return Err(Error::invalid("Episode parent must be an Episode"));
                };
                if parent.track != content.track
                    || !contains_range(&parent.experience, &content.experience)
                {
                    return Err(Error::invalid(
                        "Episode child must share parent track and lie inside its experience span",
                    ));
                }
            }
            let rows=sqlx::query("SELECT r.* FROM structured_objects o JOIN structured_revisions r ON r.object_id=o.id AND r.revision=o.current_revision WHERE o.namespace_id=? AND o.subject_id=? AND o.kind='episode' AND r.deleted=0 AND o.id<>?")
                .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.id.as_str()).fetch_all(&mut *conn).await.map_err(storage_error)?;
            for row in rows {
                let sibling = decode(conn, row).await?;
                let CognitiveContent::Episode(sibling) = sibling.body else {
                    continue;
                };
                let same_parent = match (&content.parent, &sibling.parent) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.id == b.id && a.revision == b.revision,
                    _ => false,
                };
                if content.track == sibling.track
                    && same_parent
                    && overlaps(&content.experience, &sibling.experience)
                {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "same-track Episode sibling experience spans overlap",
                    ));
                }
            }
        }
        CognitiveContent::Journal(content) => {
            if content.knowledge_cutoff < 0 || content.knowledge_cutoff >= value.commit_seq {
                return Err(Error::invalid(
                    "Journal cutoff must name authority committed before this operation",
                ));
            }
            let (nodes, _) = resolve_sources(
                conn,
                &value.namespace_id,
                &value.subject_id,
                value.sources(),
                false,
            )
            .await?;
            if nodes.iter().any(|node| {
                node.commit_seq
                    .is_none_or(|seq| seq > content.knowledge_cutoff)
            }) {
                return Err(Error::invalid(
                    "Journal source closure lies after its knowledge cutoff or has a gap",
                ));
            }
        }
        CognitiveContent::Schema(content)
            if content.formation_mode == nous_domain::FormationMode::Synthesized =>
        {
            require_independent_roots(conn, value, content.support.clone()).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn require_independent_roots(
    conn: &mut SqliteConnection,
    value: &StructuredObject,
    sources: Vec<EvidenceSource>,
) -> Result<()> {
    let (nodes, roots) =
        resolve_sources(conn, &value.namespace_id, &value.subject_id, sources, true).await?;
    if nodes
        .iter()
        .any(|node| node.status == SourceStatus::Missing)
        || roots
            .iter()
            .filter(|root| root.status == SourceStatus::Available)
            .count()
            < 2
    {
        return Err(Error::invalid(
            "synthesized cognition needs two independent available positive source roots",
        ));
    }
    Ok(())
}

fn contains_range(parent: &TimeRange, child: &TimeRange) -> bool {
    parent.domain == child.domain
        && match (parent.start, parent.end, child.start, child.end) {
            (Some(a), Some(b), Some(c), Some(d)) => a <= c && d <= b,
            _ => false,
        }
}
fn overlaps(a: &TimeRange, b: &TimeRange) -> bool {
    a.domain == b.domain
        && match (a.start, a.end, b.start, b.end) {
            (Some(a), Some(b), Some(c), Some(d)) => a < d && c < b,
            _ => false,
        }
}

async fn sync_aliases(conn: &mut SqliteConnection, value: &StructuredObject) -> Result<()> {
    let CognitiveContent::Concept(content) = &value.body else {
        return Ok(());
    };
    sqlx::query("DELETE FROM concept_aliases WHERE concept_id=?")
        .bind(value.id.as_str())
        .execute(&mut *conn)
        .await
        .map_err(write_error)?;
    if value.deleted || content.canonical.is_some() {
        return Ok(());
    }
    let mut aliases = HashSet::new();
    for alias in std::iter::once(&content.label).chain(&content.aliases) {
        let key = alias.trim().to_lowercase();
        if !aliases.insert(key.clone()) {
            continue;
        }
        sqlx::query("INSERT INTO concept_aliases(namespace_id,subject_id,alias_key,concept_id,revision) VALUES (?,?,?,?,?)")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(key).bind(value.id.as_str()).bind(value.revision)
            .execute(&mut *conn).await.map_err(write_error)?;
    }
    Ok(())
}

fn enum_name<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::invalid("expected enum name"))
}
fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        value.into(),
    ))?)
}
