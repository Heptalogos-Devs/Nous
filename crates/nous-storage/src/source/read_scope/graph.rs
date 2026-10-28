use super::*;
use crate::source::{closure::source_node, references::*};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Node {
    Cognition(CognitiveRef),
    Media(MediaReference, Option<ObservationId>),
    Observation(ObservationId),
    EventMetadata(ObservationId),
    Work(context_work::WorkContextId),
}
impl Node {
    pub(super) fn selected(reference: &SourceReadRef) -> Self {
        match reference {
            SourceReadRef::Cognition { reference } => Self::Cognition(reference.clone()),
            SourceReadRef::Media {
                reference,
                observation_id,
            } => Self::Media(reference.clone(), observation_id.clone()),
            SourceReadRef::Observation { id } => Self::Observation(id.clone()),
            SourceReadRef::WorkContext { id } => Self::Work(id.clone()),
        }
    }
    pub(super) fn target(target: &RetrievalTarget) -> Self {
        match target {
            RetrievalTarget::Cognition(reference) => Self::Cognition(reference.clone()),
            RetrievalTarget::Media(reference) => Self::Media(reference.clone(), None),
        }
    }
}

pub(super) async fn observation(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    id: &ObservationId,
) -> Result<Observation> {
    let value = crate::authority::records::observation(conn, ns, id)
        .await?
        .ok_or_else(crate::not_found)?;
    if value.subject_id != *subject {
        return Err(denied());
    }
    Ok(value)
}

fn evidence_node(source: EvidenceSource) -> Node {
    match source {
        EvidenceSource::Material { material_id } => {
            Node::Media(MediaReference::Original { material_id }, None)
        }
        EvidenceSource::Media { reference } => Node::Media(reference, None),
        EvidenceSource::Observation { observation_id } => Node::Observation(observation_id),
        EvidenceSource::MemoryRevision {
            memory_id,
            revision,
        } => Node::Cognition(CognitiveRef::memory(&memory_id, revision)),
        EvidenceSource::CognitiveRevision { reference } => Node::Cognition(reference),
    }
}

pub(super) async fn children(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    node: &Node,
    purpose: ReadPurpose,
) -> Result<Vec<Node>> {
    match node {
        Node::Cognition(reference) => {
            let (status, _, sources) =
                source_node(conn, ns, subject, &source_reference(reference), false, None).await?;
            if status == SourceStatus::Missing
                || (status == SourceStatus::Purged && purpose == ReadPurpose::Body)
                || sources.is_empty()
            {
                return Err(crate::not_found());
            }
            if reference.kind == CognitiveKind::Memory {
                let actual = sources
                    .iter()
                    .find_map(|source| match source {
                        EvidenceSource::Observation { observation_id } => {
                            Some(observation_id.clone())
                        }
                        _ => None,
                    })
                    .ok_or_else(|| {
                        Error::internal("memory source is missing its actual source event")
                    })?;
                return Ok(sources
                    .into_iter()
                    .filter_map(|source| match source {
                        EvidenceSource::Media { reference } => {
                            Some(Node::Media(reference, Some(actual.clone())))
                        }
                        EvidenceSource::Observation { .. } => None,
                        other => Some(evidence_node(other)),
                    })
                    .collect::<Vec<_>>());
            }
            Ok(sources.into_iter().map(evidence_node).collect())
        }
        Node::Observation(id) | Node::EventMetadata(id) => {
            observation(conn, ns, subject, id).await?;
            Ok(Vec::new())
        }
        Node::Work(id) => {
            let value = crate::context::work(conn, id).await?;
            if value.namespace_id != *ns || value.subject_id != *subject {
                return Err(denied());
            }
            Ok(Vec::new())
        }
        Node::Media(reference, actual) => {
            let lineage = media_source(conn, ns, subject, reference).await?;
            let event = if let Some(id) = actual {
                let event = observation(conn, ns, subject, id).await?;
                if !lineage
                    .materials
                    .iter()
                    .any(|root| root.material.id == event.material_id)
                {
                    return Err(denied());
                }
                Some(event)
            } else {
                None
            };
            let inputs = match reference {
                MediaReference::Original { .. } | MediaReference::SourceRegion { .. } => {
                    return Ok(Vec::new());
                }
                MediaReference::DerivedRegion { id } => {
                    let region = lineage
                        .derived_regions
                        .iter()
                        .find(|value| value.id == *id)
                        .ok_or_else(crate::not_found)?;
                    vec![MediaReference::Representation {
                        reference: region.representation.clone(),
                    }]
                }
                MediaReference::Representation { reference } => {
                    let value = lineage
                        .representations
                        .iter()
                        .find(|value| value.reference() == *reference)
                        .ok_or_else(crate::not_found)?;
                    let event = if let Some(id) = &value.observation_id {
                        Some(observation(conn, ns, subject, id).await?)
                    } else {
                        event
                    };
                    // Timeline locates clocks; only actual supplied inputs carry body dependencies.
                    let mut children = Vec::new();
                    let primary = value
                        .observation_id
                        .as_ref()
                        .map(|id| Node::EventMetadata(id.clone()));
                    for input in &value.inputs {
                        let roots =
                            crate::media::roots_in(conn, ns, subject, &input.source).await?;
                        let id = event
                            .as_ref()
                            .filter(|event| roots.contains(&event.material_id))
                            .map(|event| event.id.clone());
                        children.push(Node::Media(input.source.clone(), id));
                    }
                    if let Some(primary) = primary {
                        children.push(primary);
                    }
                    if let Some(id) = actual {
                        let event = Node::EventMetadata(id.clone());
                        if !children.contains(&event) {
                            children.push(event);
                        }
                    }
                    if children.is_empty() {
                        return Err(crate::not_found());
                    }
                    return Ok(children);
                }
            };
            Ok(inputs
                .into_iter()
                .map(|reference| Node::Media(reference, actual.clone()))
                .collect())
        }
    }
}

pub(super) async fn walk(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    start: Node,
    purpose: ReadPurpose,
) -> Result<Vec<(Node, Vec<Node>)>> {
    let mut pending = vec![(start, false, 0usize)];
    let mut active = BTreeSet::new();
    let mut visited = BTreeMap::new();
    while let Some((node, exit, depth)) = pending.pop() {
        if exit {
            active.remove(&node);
            continue;
        }
        if active.contains(&node) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "source-read graph contains a cycle",
            ));
        }
        if visited.contains_key(&node) {
            continue;
        }
        if depth > 64 || visited.len() >= MAX_SOURCE_NODES {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "source-read graph exceeds 64 levels or 1024 nodes",
            ));
        }
        let next = children(conn, ns, subject, &node, purpose).await?;
        active.insert(node.clone());
        visited.insert(node.clone(), next.clone());
        pending.push((node, true, depth));
        for child in next.into_iter().rev() {
            pending.push((child, false, depth + 1));
        }
    }
    Ok(visited.into_iter().collect())
}
use crate::SourceStatus;
