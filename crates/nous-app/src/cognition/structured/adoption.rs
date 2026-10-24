use super::*;
use crate::cognition::structured::model::{RawStructuredProposal, StructuredFormationInput};
use nous_storage::{StructuredFormationRun, StructuredGroupState};
use std::collections::{BTreeMap, BTreeSet, HashMap};
impl App {
    pub(super) async fn adopt_structured_groups(
        &self,
        run: &StructuredFormationRun,
        call: &ModelCall,
        proposal: &RawStructuredProposal,
    ) -> Result<()> {
        if self
            .database
            .structured_formation(&run.job.id)
            .await?
            .is_retired()
        {
            return Ok(());
        }
        let metadata: crate::common::models::InvocationMetadata<StructuredFormationInput> =
            serde_json::from_value(call.request.clone())?;
        let input = metadata.input;
        let accepted: StructuredFormationInput = serde_json::from_value(run.input.clone())?;
        if input.namespace_id != accepted.namespace_id
            || input.subject_id != accepted.subject_id
            || input.slots.len() != accepted.slots.len()
            || input
                .slots
                .iter()
                .zip(&accepted.slots)
                .any(|(actual, expected)| {
                    actual.key != expected.key
                        || actual.id != expected.id
                        || actual.kind != expected.kind
                        || actual.expected_revision != expected.expected_revision
                })
        {
            return Err(Error::invalid(
                "producer input differs from accepted formation targets",
            ));
        }
        let slots = input
            .slots
            .iter()
            .map(|s| (s.key.clone(), s))
            .collect::<HashMap<_, _>>();
        let owners = proposal
            .changes
            .iter()
            .filter_map(|(slot, change)| {
                change
                    .as_ref()
                    .map(|change| (slot.clone(), change.group_key.clone()))
            })
            .collect::<HashMap<_, _>>();
        let call_id = &call.id;
        let formed = chrono::DateTime::parse_from_rfc3339(
            call.completed_at
                .as_deref()
                .ok_or_else(|| Error::internal("producer timestamp missing"))?,
        )
        .map_err(|e| Error::internal(e.to_string()))?
        .timestamp_millis();
        let mut ancestors = HashMap::<String, BTreeSet<String>>::new();
        for group in &proposal.groups {
            let mut allowed = BTreeSet::from([group.key.clone()]);
            for dependency in &group.depends_on {
                allowed.insert(dependency.clone());
                if let Some(previous) = ancestors.get(dependency) {
                    allowed.extend(previous.clone());
                }
            }
            ancestors.insert(group.key.clone(), allowed.clone());
            let states = self
                .database
                .structured_formation_groups(&run.job.id)
                .await?;
            let mut state = states
                .iter()
                .find(|s| s.key == group.key)
                .cloned()
                .ok_or_else(|| Error::internal("proposal group missing"))?;
            if state.state != StructuredGroupState::Pending {
                continue;
            }
            let failed_dependency = group.depends_on.iter().any(|key| {
                states
                    .iter()
                    .any(|s| &s.key == key && s.state != StructuredGroupState::Applied)
            });
            let result = if failed_dependency {
                Err(Error::new(
                    ErrorCode::Conflict,
                    "dependent proposal group was not applied",
                ))
            } else {
                match group.compile(&input, &proposal.changes) {
                    Err(error) => Err(error),
                    Ok(compiled) => {
                        self.apply_structured_group(
                            run, &compiled, &slots, &owners, &allowed, call_id, formed,
                        )
                        .await
                    }
                }
            };
            if let Err(error) = result {
                if matches!(
                    error.code,
                    ErrorCode::Internal | ErrorCode::DependencyUnavailable
                ) {
                    return Err(error);
                }
                state.state = if failed_dependency {
                    StructuredGroupState::Skipped
                } else if error.code == ErrorCode::Conflict {
                    StructuredGroupState::Conflict
                } else {
                    StructuredGroupState::Rejected
                };
                state.error = Some(error);
                if let Some(mut tx) = self
                    .worker_transaction(
                        "cognition.group.reject",
                        &format!("{}:{}", run.job.id, group.key),
                        &run.job.id,
                        &json!({"group":group.key}),
                    )
                    .await?
                {
                    if tx.structured_formation(&run.job.id).await?.is_retired() {
                        tx.rollback().await?;
                        return Ok(());
                    }
                    state.commit_seq = Some(tx.commit_seq());
                    tx.update_structured_group(&run.job.id, &state).await?;
                    tx.commit(json!(state)).await?;
                }
            }
        }
        if let Some(mut tx) = self
            .worker_transaction(
                "cognition.formation.finish",
                &run.job.id,
                &run.job.id,
                &json!({"job_id":run.job.id}),
            )
            .await?
        {
            if tx.structured_formation(&run.job.id).await?.is_retired() {
                tx.rollback().await?;
                return Ok(());
            }
            let mut current = tx.structured_formation(&run.job.id).await?;
            let groups = tx.structured_formation_groups(&run.job.id).await?;
            let mut objects = Vec::new();
            for group in &groups {
                if group.state == StructuredGroupState::Applied {
                    let outcome = group
                        .result
                        .as_ref()
                        .ok_or_else(|| Error::internal("adopted group has no result"))?;
                    let adopted: Vec<StructuredObject> =
                        serde_json::from_value(outcome["objects"].clone())?;
                    objects.extend(adopted.iter().map(StructuredObject::reference));
                }
            }
            let result = serde_json::to_value(super::formation::FormationOutcome {
                model_call_id: call.id.clone(),
                group_keys: groups.iter().map(|group| group.key.clone()).collect(),
                objects,
            })?;
            self.tasks
                .complete_in(&mut tx, &mut current.job, result.clone())
                .await?;
            tx.commit(result).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_structured_group(
        &self,
        run: &StructuredFormationRun,
        group: &StructuredProposalGroup,
        slots: &HashMap<String, &StructuredTargetSlot>,
        owners: &HashMap<String, String>,
        allowed: &BTreeSet<String>,
        call_id: &ModelCallId,
        formed: i64,
    ) -> Result<()> {
        let (namespace, subject) = run.job.subject_scope()?;
        group.validate()?;
        let outputs = slots
            .iter()
            .map(|(key, slot)| Ok((slot.output_reference()?, key)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for change in &group.changes {
            for source in change.sources() {
                if let Some(reference) = source.cognitive_ref()
                    && let Some(key) = outputs.get(&reference)
                {
                    let owner = owners.get(*key).ok_or_else(|| {
                        Error::invalid("proposal refers to an unused target slot")
                    })?;
                    if !allowed.contains(owner) {
                        return Err(Error::invalid(
                            "cross-group target reference needs a declared dependency",
                        ));
                    }
                }
            }
        }
        let Some(mut tx) = self
            .worker_transaction(
                "cognition.group.apply",
                &format!("{}:{}", run.job.id, group.key),
                &run.job.id,
                &json!({"group":group.key}),
            )
            .await?
        else {
            return Ok(());
        };
        if tx.structured_formation(&run.job.id).await?.is_retired() {
            tx.rollback().await?;
            return Ok(());
        }
        let mut pending = group.changes.iter().collect::<Vec<_>>();
        let pending_targets = group
            .changes
            .iter()
            .map(|c| slots[&c.slot_key].output_reference())
            .collect::<Result<BTreeSet<_>>>()?;
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
                        .all(|r| !pending_targets.contains(&r) || saved.contains(&r))
                })
                .ok_or_else(|| {
                    Error::invalid("proposal group's object references contain a cycle")
                })?;
            let change = pending.remove(position);
            let slot = slots[&change.slot_key];
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
                    formation: TimePoint {
                        domain: "unix_ms".into(),
                        coordinate: formed,
                    },
                    validity: None,
                    recorded: self.clock.capture(),
                },
                producer_call_id: Some(call_id.clone()),
                recorded_at: self.clock.recorded(),
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
        let mut state = tx
            .structured_formation_groups(&run.job.id)
            .await?
            .into_iter()
            .find(|s| s.key == group.key)
            .ok_or_else(|| Error::internal("proposal group missing"))?;
        state.state = StructuredGroupState::Applied;
        state.commit_seq = Some(tx.commit_seq());
        state.result = Some(json!({"objects":objects}));
        tx.update_structured_group(&run.job.id, &state).await?;
        tx.commit(json!(state)).await?;
        Ok(())
    }
}
