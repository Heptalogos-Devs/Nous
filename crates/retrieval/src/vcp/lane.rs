// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_runtime::{BoundQuery, LaneOutput, LaneStatus, QueryPlan, TopologyWorkSummary};

impl ServingService {
    pub(crate) async fn vcp_lane(
        &self,
        snapshot: &ServingSnapshot,
        bound: &BoundQuery,
        plan: &QueryPlan,
        signals: &PreparedQuerySignals,
    ) -> Result<LaneOutput> {
        let mut output = LaneOutput::empty(EvidenceFamily::TopologyWave, LaneStatus::Unavailable);
        let Some(generation) = snapshot.vcp.as_ref() else {
            output
                .diagnostics
                .push("VCP serving generation is unavailable".into());
            return Ok(output);
        };
        output.generation_ref = Some(generation.generation_id);
        output.authority_watermark = Some(generation.authority_watermark);
        if !self.vcp_generation_current(bound, generation).await? {
            output
                .diagnostics
                .push("VCP generation no longer matches current Authority/capability".into());
            return Ok(output);
        }
        let Some(embedding) = signals.embedding() else {
            output
                .diagnostics
                .push("VCP requires an available permitted shared query embedding".into());
            return Ok(output);
        };
        let observation =
            match VcpQueryObservation::prepare(generation, bound, plan, embedding, Some(signals)) {
                Ok(observation) => observation,
                Err(Error::Unavailable(detail)) => {
                    output.diagnostics.push(detail);
                    return Ok(output);
                }
                Err(error) => return Err(error),
            };
        let policy = bound.config_snapshot.get(VCP_READOUT)?;
        policy.validate()?;
        let offered = offered_candidates(generation, &observation, bound, plan, signals, &policy)?;
        let limit = plan.lane_budget(EvidenceFamily::TopologyWave);
        let mut ranked = match plan.cognitive_profile {
            nous_runtime::CognitiveProfile::VcpDtsc => {
                let readout =
                    vcp_dtsc_readout(generation, &observation, &offered, &policy.dtsc, limit)?;
                output.diagnostics.push(format!(
                    "DTSC fallback={} reason={}",
                    readout.diagnostics.fallback_used,
                    readout
                        .diagnostics
                        .fallback_reason
                        .as_deref()
                        .unwrap_or("none")
                ));
                readout
                    .results
                    .into_iter()
                    .map(|score| {
                        Ok((
                            score.id,
                            serde_json::to_value(score)
                                .map_err(|e| Error::Infrastructure(e.to_string()))?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            nous_runtime::CognitiveProfile::VcpRiverMemo => {
                let readout =
                    vcp_v3_readout(generation, &observation, &offered, &policy.v3, limit)?;
                output.diagnostics.push(format!(
                    "V3 selected={} omega={}",
                    readout.selected.len(),
                    readout.omega.omega
                ));
                readout
                    .results
                    .into_iter()
                    .map(|score| {
                        Ok((
                            score.id,
                            serde_json::to_value(score)
                                .map_err(|e| Error::Infrastructure(e.to_string()))?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            _ => return Err(Error::Invalid("VCP lane received a non-VCP profile".into())),
        };
        crate::vcp::routes::explain_routes(generation, &observation, &mut ranked)?;
        // A fallback artifact or concurrent Authority mutation must not supply
        // a supposedly current graph observation. Runtime revalidates results too.
        if !self.vcp_generation_current(bound, generation).await? {
            output
                .diagnostics
                .push("Authority changed during VCP observation/readout".into());
            return Ok(output);
        }
        output.candidates = vcp_lane_candidates(
            &generation.identities,
            &ranked,
            plan.cognitive_profile.id(),
            limit,
        )?;
        let sense = &observation.numerical().sense;
        let complete = sense.diagnostics.state_truncations == 0 && !sense.transitions_truncated;
        if sense.diagnostics.state_truncations > 0 {
            output.diagnostics.push(format!(
                "VCP state truncations={}; discarded state mass is not measured",
                sense.diagnostics.state_truncations
            ));
        }
        output.diagnostics.push(format!(
            "VCP fields local_converged={} transfer_converged={} observation={}",
            observation.numerical().fields.local_converged,
            observation.numerical().fields.transfer_converged,
            observation.config_subset_digest()
        ));
        output.status = if complete {
            LaneStatus::Ready
        } else {
            LaneStatus::Truncated
        };
        output.topology_work = Some(work_summary(&observation, plan, complete));
        Ok(output)
    }
    async fn vcp_generation_current(
        &self,
        bound: &BoundQuery,
        generation: &VcpServingGeneration,
    ) -> Result<bool> {
        let seq =
            sqlx::query_scalar::<_, i64>("SELECT authority_seq FROM subjects WHERE subject_id=$1")
                .bind(bound.source_query.subject.0)
                .fetch_one(self.store.pool())
                .await
                .map_err(nous_persistence::database_error)?;
        let capabilities = self
            .projection_capabilities(bound.source_query.subject)
            .await?;
        Ok(
            (bound.historical_authority.is_some() || seq == generation.authority_watermark)
                && (capabilities.memory
                    || !generation.identities.references().iter().any(|r| {
                        matches!(
                            r,
                            CognitiveRef::Memory(_)
                                | CognitiveRef::MemoryRevision(_)
                                | CognitiveRef::EpisodeRevision(_)
                                | CognitiveRef::JournalRevision(_)
                                | CognitiveRef::CognitiveSchemaRevision(_)
                        )
                    })),
        )
    }
}

fn offered_candidates(
    generation: &VcpServingGeneration,
    observation: &VcpQueryObservation,
    bound: &BoundQuery,
    plan: &QueryPlan,
    signals: &PreparedQuerySignals,
    policy: &VcpReadoutPolicy,
) -> Result<Vec<VcpReadoutCandidate>> {
    let n = observation.numerical();
    let mut offered = BTreeMap::<i64, VcpReadoutCandidate>::new();
    let search_limit = policy
        .v3
        .pool
        .max_union_candidates
        .max(plan.lane_budget(EvidenceFamily::TopologyWave))
        .min(generation.curves.len());
    let enhanced = n
        .fusion
        .vector
        .iter()
        .map(|v| *v as f32)
        .collect::<Vec<_>>();
    let queries = if plan.cognitive_profile == nous_runtime::CognitiveProfile::VcpRiverMemo {
        vec![
            observation.original_vector(),
            enhanced.as_slice(),
            n.local_vector.as_slice(),
            n.transfer_vector.as_slice(),
        ]
    } else {
        vec![observation.original_vector()]
    };
    let mut add = |reference: &CognitiveRef,
                   base_score: f64,
                   bm25_score: f64,
                   anchor_score: f64|
     -> Result<()> {
        if !bound.source_query.projection.allows_reference(reference) {
            return Ok(());
        }
        let Ok(id) = generation.identities.id(reference) else {
            return Ok(());
        };
        if !generation.curves.iter().any(|curve| curve.id == id) {
            return Ok(());
        }
        offered
            .entry(id)
            .and_modify(|candidate| {
                candidate.base_score = candidate.base_score.max(base_score);
                candidate.bm25_score = candidate.bm25_score.max(bm25_score);
                candidate.anchor_score = candidate.anchor_score.max(anchor_score);
            })
            .or_insert(VcpReadoutCandidate {
                reference: reference.clone(),
                base_score,
                bm25_score,
                time_score: 0.0,
                anchor_score,
                self_evidence_roots: Default::default(),
            });
        Ok(())
    };
    for query in queries {
        if query.iter().all(|v| v.abs() <= f32::EPSILON) {
            continue;
        }
        for hit in generation.search_candidates_for_projection(
            query,
            search_limit,
            &bound.source_query.projection,
        )? {
            if let Some(record) = hit.record {
                add(
                    &record.reference,
                    1.0 / (1.0 + f64::from(hit.distance)),
                    0.0,
                    0.0,
                )?;
            }
        }
    }
    for lane in signals.lexical().into_iter().chain(signals.dense()) {
        for candidate in &lane.candidates {
            let score = 1.0 / f64::from(candidate.rank.max(1));
            add(
                &candidate.reference,
                score,
                if lane.family == EvidenceFamily::Lexical {
                    score
                } else {
                    0.0
                },
                0.0,
            )?;
        }
    }
    for reference in &bound.activation.exact_refs {
        add(reference, 1.0, 0.0, 1.0)?;
    }
    let mut offered = offered.into_values().collect::<Vec<_>>();
    offered.sort_by(|a, b| {
        b.base_score
            .total_cmp(&a.base_score)
            .then_with(|| a.reference.to_string().cmp(&b.reference.to_string()))
    });
    Ok(offered)
}

fn work_summary(
    observation: &VcpQueryObservation,
    plan: &QueryPlan,
    complete: bool,
) -> TopologyWorkSummary {
    let sense = &observation.numerical().sense;
    TopologyWorkSummary {
        mechanism: plan.cognitive_profile.id().into(),
        profile_id: observation.profile_id().into(),
        profile_digest: plan.cognitive_profile.digest(),
        activated_edges: sense.edges.len(),
        max_hop_observed: sense.nodes.iter().map(|node| node.hop).max().unwrap_or(0),
        seed_count: observation.seed_ids().len(),
        visited_nodes: sense.nodes.len(),
        complete,
        discarded_mass: None,
    }
}
