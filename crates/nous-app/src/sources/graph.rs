use crate::App;
use nous_domain::{media::MediaReference, *};
use nous_storage::{SourceMemoryEntry, SourceStatus, WriteTransaction};
use std::collections::{BTreeMap, BTreeSet};

struct Removed {
    memories: BTreeSet<MemoryId>,
    materials: BTreeSet<MaterialId>,
    observations: BTreeSet<ObservationId>,
    annotations: BTreeSet<String>,
    media: BTreeMap<MediaReference, bool>,
}
impl Removed {
    fn cognition(&self, kind: CognitiveKind, id: &str) -> bool {
        match kind {
            CognitiveKind::Memory => self.memories.contains(&MemoryId(id.into())),
            CognitiveKind::MemorySemantics => self.annotations.contains(id),
            _ => false,
        }
    }
    async fn source(
        &mut self,
        tx: &mut WriteTransaction,
        ns: &NamespaceId,
        sub: &SubjectId,
        source: &EvidenceSource,
    ) -> Result<bool> {
        if let Some(reference) = source.cognitive_ref() {
            return Ok(self.cognition(reference.kind, &reference.id));
        }
        match source {
            EvidenceSource::Material { material_id } => Ok(self.materials.contains(material_id)),
            EvidenceSource::Observation { observation_id } => {
                Ok(self.observations.contains(observation_id))
            }
            EvidenceSource::Media { reference } => self.media(tx, ns, sub, reference).await,
            _ => Ok(false),
        }
    }
    async fn media(
        &mut self,
        tx: &mut WriteTransaction,
        ns: &NamespaceId,
        sub: &SubjectId,
        reference: &MediaReference,
    ) -> Result<bool> {
        if let MediaReference::Original { material_id } = reference {
            return Ok(self.materials.contains(material_id));
        }
        if let Some(selected) = self.media.get(reference) {
            return Ok(*selected);
        }
        let selected = match tx.media_roots(ns, sub, reference).await {
            Ok(roots) => !roots.is_empty() && roots.iter().all(|id| self.materials.contains(id)),
            Err(error) if error.code == ErrorCode::NotFound => false,
            Err(error) => return Err(error),
        };
        self.media.insert(reference.clone(), selected);
        Ok(selected)
    }
    async fn target(
        &mut self,
        tx: &mut WriteTransaction,
        ns: &NamespaceId,
        sub: &SubjectId,
        target: &SemanticTarget,
    ) -> Result<bool> {
        match target {
            SemanticTarget::Object { identity } => Ok(self.cognition(identity.kind, &identity.id)),
            SemanticTarget::Exact {
                reference: RetrievalTarget::Cognition(reference),
            } => Ok(self.cognition(reference.kind, &reference.id)),
            SemanticTarget::Exact {
                reference: RetrievalTarget::Media(reference),
            } => self.media(tx, ns, sub, reference).await,
        }
    }
    async fn links(
        &mut self,
        tx: &mut WriteTransaction,
        ns: &NamespaceId,
        sub: &SubjectId,
        links: &mut Vec<EvidenceLink>,
    ) -> Result<(bool, bool)> {
        let mut changed = false;
        let mut removed_support = false;
        for link in std::mem::take(links) {
            if self.source(tx, ns, sub, &link.source).await? {
                changed = true;
                removed_support |= link.polarity == EvidencePolarity::Support;
            } else {
                links.push(link);
            }
        }
        Ok((changed, removed_support))
    }
}
async fn supported(
    tx: &mut WriteTransaction,
    ns: &NamespaceId,
    sub: &SubjectId,
    links: &[EvidenceLink],
) -> Result<bool> {
    for link in links {
        if link.polarity == EvidencePolarity::Support
            && tx.evidence_source_status(ns, sub, &link.source).await? == SourceStatus::Available
        {
            return Ok(true);
        }
    }
    Ok(false)
}
impl App {
    pub(super) async fn retire_source_graph_in(
        &self,
        tx: &mut WriteTransaction,
        ns: &NamespaceId,
        sub: &SubjectId,
        records: &[SourceMemoryEntry],
    ) -> Result<SourceGraphChanges> {
        if records.is_empty() {
            return Ok(SourceGraphChanges::default());
        }
        let objects = tx.source_graph_objects(ns, sub).await?;
        let mut removed = Removed {
            memories: records.iter().map(|r| r.id.clone()).collect(),
            materials: records.iter().map(|r| r.material_id.clone()).collect(),
            observations: records.iter().map(|r| r.observation_id.clone()).collect(),
            annotations: BTreeSet::new(),
            media: BTreeMap::new(),
        };
        let mut changes = SourceGraphChanges::default();
        for object in &objects {
            if let CognitiveContent::MemorySemantics(annotation) = &object.body
                && removed.memories.contains(&annotation.memory_id)
            {
                removed.annotations.insert(object.id.to_string());
                let mut retired = object.clone();
                retired.deleted = true;
                changes
                    .retired_annotations
                    .push(self.save_source_graph_revision(tx, retired).await?);
            }
        }
        let mut endpoints = BTreeMap::new();
        for mut object in objects
            .iter()
            .filter(|v| matches!(v.body, CognitiveContent::Entity(_)))
            .cloned()
        {
            let previous = object.reference();
            let (mut changed, _) = removed.links(tx, ns, sub, &mut object.evidence).await?;
            let CognitiveContent::Entity(entity) = &mut object.body else {
                unreachable!()
            };
            for mention in std::mem::take(&mut entity.mentions) {
                if removed.target(tx, ns, sub, &mention.target).await? {
                    changed = true;
                } else {
                    entity.mentions.push(mention);
                }
            }
            for mut attribute in std::mem::take(&mut entity.attributes) {
                let (touched, _) = removed.links(tx, ns, sub, &mut attribute.evidence).await?;
                changed |= touched;
                if !touched || supported(tx, ns, sub, &attribute.evidence).await? {
                    entity.attributes.push(attribute);
                }
            }
            if changed {
                let current = self.save_source_graph_revision(tx, object).await?;
                endpoints.insert(previous, current.clone());
                changes.revised_entities.push(current);
            }
        }
        for mut object in objects
            .into_iter()
            .filter(|v| matches!(v.body, CognitiveContent::Association(_)))
        {
            let (mut changed, removed_support) =
                removed.links(tx, ns, sub, &mut object.evidence).await?;
            let has_support = supported(tx, ns, sub, &object.evidence).await?;
            let CognitiveContent::Association(association) = &mut object.body else {
                unreachable!()
            };
            // Only replace an endpoint bound to the exact current revision we revised.
            if let Some(current) = endpoints.get(&association.from) {
                association.from = current.clone();
                changed = true;
            }
            if let Some(current) = endpoints.get(&association.to) {
                association.to = current.clone();
                changed = true;
            }
            if removed_support && !has_support {
                object.deleted = true;
            }
            if changed {
                let deleted = object.deleted;
                let reference = self.save_source_graph_revision(tx, object).await?;
                if deleted {
                    changes.retired_associations.push(reference);
                } else {
                    changes.revised_associations.push(reference);
                }
            }
        }
        Ok(changes)
    }
    async fn save_source_graph_revision(
        &self,
        tx: &mut WriteTransaction,
        mut object: StructuredObject,
    ) -> Result<CognitiveRef> {
        let old = object.reference();
        let affected = tx
            .cognition_source_dependents(&object.namespace_id, &object.subject_id, &old)
            .await?;
        tx.invalidate_context_history(&object.namespace_id, &object.subject_id, &affected)
            .await?;
        if !object.deleted {
            tx.retire_embedding_targets(
                &object.namespace_id,
                &object.subject_id,
                &[RetrievalTarget::Cognition(old)],
            )
            .await?;
        }
        let expected = object.revision;
        object.revision = expected
            .checked_add(1)
            .ok_or_else(|| Error::invalid("source graph revision overflow"))?;
        object.commit_seq = tx.commit_seq();
        object.recorded_at = tx.time().recorded();
        object.times.recorded = tx.time().point();
        object.producer_call_id = None;
        tx.save_structured(&object, Some(expected)).await?;
        Ok(object.reference())
    }
}
