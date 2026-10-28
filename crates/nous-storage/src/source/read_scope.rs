use crate::{Database, ReadView, WriteTransaction, storage_error};
use nous_domain::{media::MediaReference, *};
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) mod candidates;
mod graph;
mod imports;
use graph::Node;
const MAX_SOURCE_NODES: usize = 1024;

/// Ephemeral metadata for the same Authority snapshot, never an additional grant.
pub struct SourceReadView {
    namespace: NamespaceId,
    subject: SubjectId,
    constraints: Vec<Selection>,
    purpose: ReadPurpose,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadPurpose {
    Body,
    Provenance,
}
impl SourceReadView {
    pub fn is_restricted(&self) -> bool {
        !self.constraints.is_empty()
    }
}
struct Selection {
    nodes: BTreeSet<Node>,
    whole_inputs: BTreeSet<MaterialId>,
    origins: BTreeSet<ConversationRef>,
}
fn denied() -> Error {
    Error::new(
        ErrorCode::Forbidden,
        "source body is outside the bound source-read scope",
    )
}

async fn bind(
    conn: &mut SqliteConnection,
    ctx: &CallContext,
    ns: &NamespaceId,
    subject: &SubjectId,
    purpose: ReadPurpose,
) -> Result<SourceReadView> {
    ctx.require_subject(ns, subject)?;
    crate::authority::records::subject(conn, ns, subject)
        .await?
        .ok_or_else(crate::not_found)?;
    let mut constraints = Vec::new();
    for scope in &ctx.source_read {
        scope.validate()?;
        if scope.subject_id != *subject {
            return Err(denied());
        }
        let mut selection = Selection {
            nodes: BTreeSet::new(),
            whole_inputs: scope
                .source_refs
                .iter()
                .filter_map(|reference| match reference {
                    SourceReadRef::Media {
                        reference: MediaReference::Original { material_id },
                        observation_id: None,
                    } => Some(material_id.clone()),
                    _ => None,
                })
                .collect(),
            origins: scope.origin_conversation_refs.iter().cloned().collect(),
        };
        for reference in &scope.source_refs {
            for (node, _) in
                graph::walk(conn, ns, subject, Node::selected(reference), purpose).await?
            {
                selection.nodes.insert(node);
                if selection.nodes.len() > MAX_SOURCE_NODES {
                    return Err(Error::new(
                        ErrorCode::ResourceExhausted,
                        "explicit source-read metadata exceeds 1024 nodes",
                    ));
                }
            }
        }
        constraints.push(selection);
    }
    Ok(SourceReadView {
        namespace: ns.clone(),
        subject: subject.clone(),
        constraints,
        purpose,
    })
}

async fn occurrence_allowed(
    conn: &mut SqliteConnection,
    view: &SourceReadView,
    selection: &Selection,
    id: &ObservationId,
) -> Result<bool> {
    let observation = graph::observation(conn, &view.namespace, &view.subject, id).await?;
    if selection.nodes.contains(&Node::Observation(id.clone()))
        || selection.nodes.contains(&Node::EventMetadata(id.clone()))
        || selection.nodes.iter().any(|node| {
            matches!(node,
            Node::Media(_, Some(actual)) if actual == id)
        })
        || observation
            .origin
            .as_ref()
            .and_then(|origin| origin.conversation_ref.as_ref())
            .is_some_and(|origin| selection.origins.contains(origin))
    {
        return Ok(true);
    }
    if observation.kind != ObservationKind::Import {
        return Ok(false);
    }
    imports::allowed(conn, view, selection, &observation.material_id, Some(id)).await
}

async fn media_leaf_allowed(
    conn: &mut SqliteConnection,
    view: &SourceReadView,
    selection: &Selection,
    reference: &MediaReference,
    actual: &Option<ObservationId>,
) -> Result<bool> {
    if selection
        .nodes
        .contains(&Node::Media(reference.clone(), actual.clone()))
    {
        return Ok(true);
    }
    let lineage =
        crate::source::references::media_source(conn, &view.namespace, &view.subject, reference)
            .await?;
    let material = match reference {
        MediaReference::Original { material_id } => material_id,
        MediaReference::SourceRegion { id } => {
            &lineage
                .source_regions
                .iter()
                .find(|region| region.id == *id)
                .ok_or_else(crate::not_found)?
                .material_id
        }
        _ => {
            return Err(Error::internal(
                "nonphysical media became a source-read leaf",
            ));
        }
    };
    let original = MediaReference::Original {
        material_id: material.clone(),
    };
    let locator_explicit = selection
        .nodes
        .contains(&Node::Media(reference.clone(), None))
        || selection.nodes.contains(&Node::Media(original, None));
    if let Some(actual) = actual {
        // A locator used by a cognition keeps its actual event, even for equal CAS bytes.
        return occurrence_allowed(conn, view, selection, actual).await;
    }
    if locator_explicit {
        return Ok(true);
    }
    if imports::allowed(conn, view, selection, material, None).await? {
        return Ok(true);
    }
    let rows = sqlx::query(
        "SELECT origin FROM observations WHERE namespace_id=? AND subject_id=? AND material_id=?",
    )
    .bind(view.namespace.as_str())
    .bind(view.subject.as_str())
    .bind(material.as_str())
    .fetch_all(conn)
    .await
    .map_err(storage_error)?;
    let mut origins = BTreeSet::new();
    if rows.is_empty() {
        return Ok(false);
    }
    for row in rows {
        let Some(raw) = row.get::<Option<&str>, _>("origin") else {
            return Ok(false);
        };
        let origin: ObservationOrigin = serde_json::from_str(raw)?;
        let Some(conversation) = origin.conversation_ref else {
            return Ok(false);
        };
        if !selection.origins.contains(&conversation) {
            return Ok(false);
        }
        origins.insert(conversation);
    }
    Ok(origins.len() == 1)
}

pub(crate) async fn target_allowed(
    conn: &mut SqliteConnection,
    view: &SourceReadView,
    target: &RetrievalTarget,
) -> Result<bool> {
    allowed(conn, view, Node::target(target)).await
}
async fn allowed(conn: &mut SqliteConnection, view: &SourceReadView, node: Node) -> Result<bool> {
    if view.constraints.is_empty() {
        return Ok(true);
    }
    let graph = match graph::walk(conn, &view.namespace, &view.subject, node, view.purpose).await {
        Ok(graph) => graph,
        Err(error) if error.code == ErrorCode::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for selection in &view.constraints {
        for (node, children) in &graph {
            if !children.is_empty() {
                continue;
            }
            let permitted = match node {
                Node::Observation(id) => occurrence_allowed(conn, view, selection, id).await?,
                Node::EventMetadata(id) => {
                    selection.nodes.contains(node)
                        || occurrence_allowed(conn, view, selection, id).await?
                }
                Node::Work(_) => selection.nodes.contains(node),
                Node::Media(reference, actual) => {
                    media_leaf_allowed(conn, view, selection, reference, actual).await?
                }
                Node::Cognition(_) => false,
            };
            if !permitted {
                return Ok(false);
            }
        }
    }
    Ok(!graph.is_empty())
}

macro_rules! source_reads {
    ($owner:ty) => {
        impl $owner {
            pub async fn source_read_view(
                &mut self,
                ctx: &CallContext,
                namespace: &NamespaceId,
                subject: &SubjectId,
            ) -> Result<SourceReadView> {
                bind(&mut self.tx, ctx, namespace, subject, ReadPurpose::Body).await
            }
            pub async fn source_read_allowed(
                &mut self,
                view: &SourceReadView,
                target: &RetrievalTarget,
            ) -> Result<bool> {
                allowed(&mut self.tx, view, Node::target(target)).await
            }
            pub async fn require_source_read(
                &mut self,
                ctx: &CallContext,
                namespace: &NamespaceId,
                subject: &SubjectId,
                target: &RetrievalTarget,
            ) -> Result<()> {
                let view = self.source_read_view(ctx, namespace, subject).await?;
                if self.source_read_allowed(&view, target).await? {
                    Ok(())
                } else {
                    Err(denied())
                }
            }
            pub async fn require_observation_read(
                &mut self,
                ctx: &CallContext,
                namespace: &NamespaceId,
                subject: &SubjectId,
                id: &ObservationId,
            ) -> Result<()> {
                let view = self.source_read_view(ctx, namespace, subject).await?;
                if allowed(&mut self.tx, &view, Node::Observation(id.clone())).await? {
                    Ok(())
                } else {
                    Err(denied())
                }
            }
            pub async fn require_work_context_read(
                &mut self,
                ctx: &CallContext,
                namespace: &NamespaceId,
                subject: &SubjectId,
                id: &context_work::WorkContextId,
            ) -> Result<()> {
                let view = self.source_read_view(ctx, namespace, subject).await?;
                if allowed(&mut self.tx, &view, Node::Work(id.clone())).await? {
                    Ok(())
                } else {
                    Err(denied())
                }
            }
            pub async fn require_source_requirements(
                &mut self,
                ctx: &CallContext,
                namespace: &NamespaceId,
                subject: &SubjectId,
                references: &[SourceReadRef],
            ) -> Result<()> {
                let view = self.source_read_view(ctx, namespace, subject).await?;
                for reference in references {
                    reference.validate()?;
                    graph::children(
                        &mut self.tx,
                        namespace,
                        subject,
                        &Node::selected(reference),
                        ReadPurpose::Body,
                    )
                    .await?;
                    if !allowed(&mut self.tx, &view, Node::selected(reference)).await? {
                        return Err(denied());
                    }
                }
                Ok(())
            }
            /// Authorize retained source facts without making an erased body readable.
            pub async fn require_source_provenance(
                &mut self,
                ctx: &CallContext,
                namespace: &NamespaceId,
                subject: &SubjectId,
                references: &[SourceReadRef],
            ) -> Result<()> {
                let view = bind(
                    &mut self.tx,
                    ctx,
                    namespace,
                    subject,
                    ReadPurpose::Provenance,
                )
                .await?;
                for reference in references {
                    reference.validate()?;
                    graph::walk(
                        &mut self.tx,
                        namespace,
                        subject,
                        Node::selected(reference),
                        ReadPurpose::Provenance,
                    )
                    .await?;
                    if !allowed(&mut self.tx, &view, Node::selected(reference)).await? {
                        return Err(denied());
                    }
                }
                Ok(())
            }
        }
    };
}
source_reads!(ReadView);
source_reads!(WriteTransaction);
impl Database {
    pub async fn require_source_read(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
    ) -> Result<()> {
        ctx.require_subject(namespace, subject)?;
        if ctx.source_read.is_empty() {
            return Ok(());
        }
        let mut view = self.read_view().await?;
        view.require_source_read(ctx, namespace, subject, target)
            .await?;
        view.finish().await
    }
}
