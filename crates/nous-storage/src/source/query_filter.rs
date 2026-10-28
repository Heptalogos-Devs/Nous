use crate::{ReadView, storage_error};
use nous_domain::{media::MediaMetadata, *};
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};

impl ReadView {
    pub async fn query_metadata_matches(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
        filters: &QueryFilters,
        as_of: Option<i64>,
    ) -> Result<bool> {
        if as_of.is_some_and(|cutoff| cutoff < 0 || cutoff > self.watermark) {
            return Err(Error::invalid(
                "query filter cutoff is outside its Authority view",
            ));
        }
        target_matches(&mut self.tx, namespace, subject, target, filters, as_of).await
    }
}
pub(crate) async fn target_matches(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    target: &RetrievalTarget,
    filters: &QueryFilters,
    as_of: Option<i64>,
) -> Result<bool> {
    filters.validate()?;
    if filters.is_empty() {
        return Ok(true);
    }
    target.validate()?;
    if filters.needs_cognition() {
        let matches = match target {
            RetrievalTarget::Cognition(reference) if reference.kind == CognitiveKind::Memory => {
                let status:Option<String>=sqlx::query_scalar("SELECT epistemic_status FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=? AND (? IS NULL OR commit_seq<=?)")
                    .bind(namespace.as_str()).bind(subject.as_str()).bind(&reference.id).bind(reference.revision).bind(as_of).bind(as_of).fetch_optional(&mut *conn).await.map_err(storage_error)?;
                status.is_some_and(|status| filters.matches_cognition(None, Some(&status)))
            }
            RetrievalTarget::Cognition(reference) => {
                let object = crate::cognition::structured::read::object(
                    conn,
                    namespace,
                    &CognitiveId(reference.id.clone()),
                    Some(reference.revision),
                )
                .await?;
                object.subject_id == *subject
                    && as_of.is_none_or(|cutoff| object.commit_seq <= cutoff)
                    && filters.matches_cognition(Some(&object.body), None)
            }
            RetrievalTarget::Media(_) => filters.matches_cognition(None, None),
        };
        if !matches {
            return Ok(false);
        }
    }
    if !filters.needs_roots() {
        return Ok(true);
    }
    let source = match target {
        RetrievalTarget::Cognition(reference) => super::references::source_reference(reference),
        RetrievalTarget::Media(reference) => EvidenceSource::Media {
            reference: reference.clone(),
        },
    };
    let (nodes, roots) =
        super::closure::resolve_sources_at(conn, namespace, subject, vec![source], false, as_of)
            .await?;
    if nodes.iter().any(|node| {
        matches!(
            node.status,
            crate::SourceStatus::Missing | crate::SourceStatus::Purged
        )
    }) {
        return Ok(false);
    }
    let mut conversations: BTreeMap<MaterialId, BTreeSet<ConversationRef>> = BTreeMap::new();
    let mut published=BTreeSet::new();
    let mut global_publications=BTreeSet::new();
    let mut explicit = BTreeSet::new();
    for node in nodes {
        if let EvidenceSource::Observation { observation_id } = node.source {
            let observation =
                crate::authority::records::observation(conn, namespace, &observation_id)
                    .await?
                    .ok_or_else(crate::not_found)?;
            if observation.subject_id != *subject
                || as_of.is_some_and(|cutoff| observation.commit_seq > cutoff)
            {
                continue;
            }
            explicit.insert(observation.material_id.clone());
            if let Some(placement)=crate::bundle::placement::observation_placement_in(conn,namespace,subject,&observation_id).await? {
                published.insert(observation.material_id.clone());
                if placement==BundlePlacement::Global {global_publications.insert(observation.material_id.clone());}
            }
            if let Some(conversation) = observation
                .origin
                .and_then(|origin| origin.conversation_ref)
            {
                conversations
                    .entry(observation.material_id)
                    .or_default()
                    .insert(conversation);
            }
        }
    }
    let mut facts = Vec::new();
    for root in roots {
        let material = crate::authority::records::material(conn, namespace, &root.material_id)
            .await?
            .ok_or_else(crate::not_found)?;
        if matches!(target, RetrievalTarget::Media(_)) && !explicit.contains(&root.material_id) {
            let rows=sqlx::query("SELECT * FROM observations WHERE namespace_id=? AND subject_id=? AND material_id=? AND (? IS NULL OR commit_seq<=?)")
                .bind(namespace.as_str()).bind(subject.as_str()).bind(root.material_id.as_str()).bind(as_of).bind(as_of).fetch_all(&mut *conn).await.map_err(storage_error)?;
            for row in rows {
                let observation_id=ObservationId(row.get("id"));
                if let Some(placement)=crate::bundle::placement::observation_placement_in(conn,namespace,subject,&observation_id).await? {
                    published.insert(root.material_id.clone());
                    if placement==BundlePlacement::Global {global_publications.insert(root.material_id.clone());}
                }
                if let Some(raw) = row.get::<Option<&str>, _>("origin") {
                    let origin: ObservationOrigin = serde_json::from_str(raw)?;
                    if let Some(conversation) = origin.conversation_ref {
                        conversations
                            .entry(root.material_id.clone())
                            .or_default()
                            .insert(conversation);
                    }
                }
            }
        }
        let details =
            super::materials::material_details(conn, namespace, &root.material_id).await?;
        let person_ids = details
            .metadata
            .get("person_ids")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let declared_global = details
            .metadata
            .get("scope_type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("global"));
        let global_source=if published.contains(&root.material_id) {global_publications.contains(&root.material_id)} else {declared_global};
        let modality = match material.metadata {
            MediaMetadata::Text => "text",
            MediaMetadata::Image { .. } => "image",
            MediaMetadata::Audio { .. } => "audio",
            MediaMetadata::Video { .. } => "video",
            MediaMetadata::Document { .. } => "document",
            MediaMetadata::Binary => "binary",
        }
        .to_owned();
        facts.push(QueryRootFacts {
            conversations: conversations
                .remove(&root.material_id)
                .unwrap_or_default()
                .into_iter()
                .collect(),
            material_id: root.material_id,
            person_ids,
            source_type: details.source_type,
            tags: details.tags,
            global_source,
            modality,
        });
    }
    Ok(filters.matches(&facts))
}
