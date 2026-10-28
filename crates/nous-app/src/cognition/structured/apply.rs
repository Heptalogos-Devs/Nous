use super::*;
use nous_storage::WriteTransaction;
use std::collections::{BTreeSet, HashMap};

pub(crate) struct StructuredGroupProvenance {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub producer_call_id: Option<ModelCallId>,
    pub formation: TimePoint,
}

impl App {
    /// Apply one compiled group inside the caller's authority transaction.
    /// The caller commits progress with these objects, or rolls back on any error.
    pub(crate) async fn apply_structured_group_in(
        &self,
        tx: &mut WriteTransaction,
        group: &StructuredProposalGroup,
        target_slots: &[StructuredTargetSlot],
        provenance: StructuredGroupProvenance,
    ) -> Result<Vec<StructuredObject>> {
        let namespace = &provenance.namespace_id;
        let subject = &provenance.subject_id;
        tx.context().require_subject(namespace, subject)?;
        provenance.formation.validate()?;
        group.validate()?;
        let mut slots = HashMap::new();
        let mut ids = BTreeSet::new();
        for slot in target_slots {
            slot.output_reference()?;
            if slots.insert(slot.key.as_str(), slot).is_some() || !ids.insert(&slot.id) {
                return Err(Error::invalid(
                    "structured target slots require unique keys and identities",
                ));
            }
        }
        let mut pending_targets = BTreeSet::new();
        for change in &group.changes {
            let slot = slots
                .get(change.slot_key.as_str())
                .ok_or_else(|| Error::invalid("compiled group selects an unknown target slot"))?;
            if change.body.kind() != slot.kind {
                return Err(Error::invalid(
                    "compiled body kind differs from its target slot",
                ));
            }
            pending_targets.insert(slot.output_reference()?);
        }
        let mut pending = group.changes.iter().collect::<Vec<_>>();
        let mut saved = BTreeSet::new();
        let mut objects = Vec::new();
        while !pending.is_empty() {
            let position = pending
                .iter()
                .position(|change| {
                    change
                        .sources()
                        .iter()
                        .filter_map(EvidenceSource::cognitive_ref)
                        .all(|reference| {
                            !pending_targets.contains(&reference) || saved.contains(&reference)
                        })
                })
                .ok_or_else(|| {
                    Error::invalid("proposal group's object references contain a cycle")
                })?;
            let change = pending.remove(position);
            let slot = slots[change.slot_key.as_str()];
            if slot.expected_revision > 0
                && tx.structured_object(namespace, &slot.id).await?.deleted
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "formation target was deleted",
                ));
            }
            let object = StructuredObject {
                id: slot.id.clone(),
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
                revision: slot.expected_revision + 1,
                body: change.body.clone(),
                evidence: change.evidence.clone(),
                times: CognitiveTimes {
                    event: None,
                    observation: None,
                    formation: provenance.formation.clone(),
                    validity: None,
                    recorded: tx.time().point(),
                },
                producer_call_id: provenance.producer_call_id.clone(),
                recorded_at: tx.time().recorded(),
                commit_seq: tx.commit_seq(),
                deleted: false,
            };
            tx.save_structured(
                &object,
                (slot.expected_revision > 0).then_some(slot.expected_revision),
            )
            .await?;
            saved.insert(object.reference());
            objects.push(object);
        }
        Ok(objects)
    }
}
