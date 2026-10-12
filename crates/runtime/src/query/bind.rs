// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::resolve_retrieval_policy;
use super::types::{BoundQuery, ExactBinding, RevisionPolicy};
use crate::CognitiveRuntimeService;
use nous_core::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

type ReferenceBindings = HashMap<CognitiveRef, (CognitiveRef, Option<i64>, bool)>;

pub fn planned_lanes(query: &CognitiveQuery) -> Vec<EvidenceFamily> {
    if query.is_exact_read() {
        return vec![EvidenceFamily::Exact];
    }
    let mut result = Vec::new();
    for node in query.scopes() {
        let mut scoped = query.clone();
        scoped.expression = node.clone();
        scoped.expression.children.clear();
        result.extend(local_lanes(&scoped));
    }
    result.sort();
    result.dedup();
    result
}

pub(super) fn planned_profile_lanes(
    query: &CognitiveQuery,
    profile: super::CognitiveProfile,
) -> Vec<EvidenceFamily> {
    let mut lanes = planned_lanes(query);
    if !profile.requirements().topology {
        lanes.retain(|family| *family != EvidenceFamily::TopologyWave);
    }
    lanes
}

fn local_lanes(query: &CognitiveQuery) -> Vec<EvidenceFamily> {
    let mut lanes = Vec::new();
    if query
        .expression
        .targets
        .iter()
        .any(|target| matches!(target, QueryTarget::Exact { .. }))
    {
        return vec![EvidenceFamily::Exact];
    }
    if query.session.is_some() || !query.situation.current_refs.is_empty() {
        lanes.push(EvidenceFamily::Runtime);
    }
    if query
        .expression
        .cues
        .iter()
        .any(|cue| matches!(cue, Cue::Entity(_)))
        || !query.expression.constraints.entity_requirements.is_empty()
        || query
            .expression
            .targets
            .iter()
            .any(|target| matches!(target, QueryTarget::EntityNeighborhood { .. }))
    {
        lanes.push(EvidenceFamily::Entity);
    }
    let has_text = query
        .expression
        .cues
        .iter()
        .any(|cue| matches!(cue, Cue::Text(_) | Cue::Example(_)));
    if has_text {
        lanes.push(EvidenceFamily::Lexical);
        if query.capabilities.text_embedding != RequirementStrength::Forbidden {
            // The lane remains planned even when its provider is unavailable.
            lanes.push(EvidenceFamily::Dense);
        }
    }
    if query
        .expression
        .cues
        .iter()
        .any(|cue| matches!(cue, Cue::Concept(_)))
        && query.capabilities.text_embedding != RequirementStrength::Forbidden
    {
        lanes.push(EvidenceFamily::Dense);
    }
    if query.expression.constraints.valid.is_some()
        || query.expression.constraints.occurred.is_some()
        || query.expression.constraints.observed.is_some()
        || query.expression.constraints.formed.is_some()
        || query.expression.constraints.recorded.is_some()
    {
        lanes.push(EvidenceFamily::Temporal);
    }
    if query
        .expression
        .cues
        .iter()
        .any(|cue| matches!(cue, Cue::Schema(_)))
        || query.expression.targets.iter().any(|target| {
            matches!(
                target,
                QueryTarget::SchemaNeighborhood { .. }
                    | QueryTarget::Exact {
                        reference: CognitiveRef::CognitiveSchema(_)
                            | CognitiveRef::CognitiveSchemaRevision(_),
                    }
            )
        })
    {
        lanes.push(EvidenceFamily::SchemaDirect);
    }
    if query.scopes().iter().any(|scope| {
        scope.cues.iter().any(|cue| matches!(cue, Cue::Tag(_)))
            || scope.targets.iter().any(|t| {
                matches!(
                    t,
                    QueryTarget::Exact {
                        reference: CognitiveRef::Tag(_)
                    }
                )
            })
    }) || query
        .situation
        .current_refs
        .iter()
        .any(|r| matches!(r, CognitiveRef::Tag(_)))
    {
        lanes.push(EvidenceFamily::TagDirect);
    }
    if explicit_topology(query) {
        lanes.push(EvidenceFamily::TopologyWave);
    }
    lanes.sort();
    lanes.dedup();
    lanes
}

fn include_concept_lane(
    query: &CognitiveQuery,
    enrichment: super::ConceptEnrichment,
    runtime_refs: &[CognitiveRef],
    lanes: &mut Vec<EvidenceFamily>,
) {
    if !query.is_exact_read()
        && (enrichment != super::ConceptEnrichment::Off
            || runtime_refs
                .iter()
                .any(|reference| matches!(reference, CognitiveRef::Tag(_))))
    {
        lanes.push(EvidenceFamily::TagDirect);
        lanes.sort();
        lanes.dedup();
    }
}

pub fn explicit_topology(query: &CognitiveQuery) -> bool {
    matches!(
        query.exploration,
        ExplorationIntent::BoundedAssociative
            | ExplorationIntent::AroundTag
            | ExplorationIntent::AroundSchema
            | ExplorationIntent::ExplainAssociation
    )
}

fn validate_hard_constraints(query: &CognitiveQuery) -> Result<()> {
    for role in &query.expression.constraints.cognitive_roles_include {
        if !matches!(
            role.as_str(),
            "experiential" | "declarative" | "procedural_experience"
        ) {
            return Err(Error::Invalid(format!(
                "unsupported cognitive role constraint: {role}"
            )));
        }
    }
    for mode in &query.expression.constraints.formation_modes_include {
        if !matches!(mode.as_str(), "grounded" | "synthesized") {
            return Err(Error::Invalid(format!(
                "unsupported formation mode constraint: {mode}"
            )));
        }
    }
    for evidence_class in &query.expression.constraints.evidence_classes {
        if !matches!(
            evidence_class.as_str(),
            "observed" | "reported" | "derived" | "inferred" | "narrative" | "simulated"
        ) {
            return Err(Error::Invalid(format!(
                "unsupported evidence class constraint: {evidence_class}"
            )));
        }
    }
    Ok(())
}

fn budget_values(
    query: &CognitiveQuery,
    lanes: &[EvidenceFamily],
    policy: &super::RetrievalPolicy,
) -> BTreeMap<EvidenceFamily, usize> {
    let index = match query.effort {
        CognitiveEffort::Light => 0,
        CognitiveEffort::Normal => 1,
        CognitiveEffort::Deep => 2,
        CognitiveEffort::Maximum => 3,
    };
    let budget = query
        .result_need
        .limit
        .saturating_mul(policy.effort_multipliers[index])
        .clamp(policy.lane_min, policy.per_lane_max[index]);
    lanes.iter().copied().map(|lane| (lane, budget)).collect()
}

impl CognitiveRuntimeService {
    async fn prepared_reference(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
        bindings: &mut ReferenceBindings,
    ) -> Result<(CognitiveRef, Option<i64>, bool)> {
        if let Some(binding) = bindings.get(reference) {
            return Ok(binding.clone());
        }
        let binding = self
            .store
            .bind_reference_in_view(subject, reference, view)
            .await?;
        bindings.insert(reference.clone(), binding.clone());
        // Tag canonicalization rewrites query cues. Revision refs keep their
        // own immutable binding semantics rather than inheriting head epochs.
        if matches!(binding.0, CognitiveRef::Tag(_)) {
            bindings
                .entry(binding.0.clone())
                .or_insert_with(|| binding.clone());
        }
        Ok(binding)
    }

    async fn canonicalize_query_tags(
        &self,
        query: &mut CognitiveQuery,
        view: Option<&HistoricalAuthoritySnapshot>,
        bindings: &mut ReferenceBindings,
    ) -> Result<()> {
        let mut tag_ids = HashSet::new();
        visit_expression_tags(&mut query.expression, &mut |tag| {
            tag_ids.insert(*tag);
        });
        let mut canonical_tags = std::collections::HashMap::new();
        for tag in tag_ids {
            let (canonical, _, _) = self
                .prepared_reference(query.subject, &CognitiveRef::Tag(tag), view, bindings)
                .await?;
            let CognitiveRef::Tag(canonical) = canonical else {
                return Err(Error::Internal("invalid canonical Tag binding".into()));
            };
            canonical_tags.insert(tag, canonical);
        }
        visit_expression_tags(&mut query.expression, &mut |tag| {
            *tag = canonical_tags[tag];
        });
        Ok(())
    }
    fn capture_query_clock(&self, query: &mut CognitiveQuery) {
        if query.temporal_frame.clock_now == chrono::DateTime::<chrono::Utc>::UNIX_EPOCH {
            query.temporal_frame.clock_now = self.now(query.subject);
        }
    }

    async fn bind_exact_targets(
        &self,
        query: &CognitiveQuery,
        view: Option<&HistoricalAuthoritySnapshot>,
        bindings: &mut ReferenceBindings,
    ) -> Result<(Vec<ExactBinding>, HashSet<CognitiveRef>)> {
        let mut exact_bindings = Vec::new();
        let mut allowed_revision_refs = HashSet::new();
        for target in query.scopes().into_iter().flat_map(|node| &node.targets) {
            let QueryTarget::Exact { reference } = target else {
                continue;
            };
            let (bound_ref, epoch, mutable_object) = self
                .prepared_reference(query.subject, reference, view, bindings)
                .await?;
            if !mutable_object
                && matches!(
                    reference,
                    CognitiveRef::MemoryRevision(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                        | CognitiveRef::EpisodeRevision(_)
                        | CognitiveRef::JournalRevision(_)
                )
            {
                allowed_revision_refs.insert(bound_ref.clone());
            }
            exact_bindings.push(ExactBinding {
                requested_ref: reference.clone(),
                bound_ref,
                bound_object_epoch: epoch,
                mutable_object,
            });
        }

        Ok((exact_bindings, allowed_revision_refs))
    }
    async fn bind_runtime_sources(
        &self,
        subject: SubjectId,
        sources: &[(CognitiveRef, String)],
        view: Option<&HistoricalAuthoritySnapshot>,
        bindings: &mut ReferenceBindings,
    ) -> Result<(Vec<CognitiveRef>, Vec<(CognitiveRef, String)>)> {
        let mut resolved = Vec::new();
        for (reference, source) in sources {
            let (canonical, _, _) = self
                .prepared_reference(subject, reference, view, bindings)
                .await?;
            resolved.push((canonical, source.clone()));
        }
        let mut refs = resolved
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        refs.sort_by_key(ToString::to_string);
        refs.dedup();
        resolved.sort_by_key(|(reference, source)| (reference.to_string(), source.clone()));
        resolved.dedup();
        Ok((refs, resolved))
    }
    async fn bind_structural_query_refs(
        &self,
        query: &CognitiveQuery,
        view: Option<&HistoricalAuthoritySnapshot>,
        bindings: &mut ReferenceBindings,
    ) -> Result<Vec<(CognitiveRef, String)>> {
        let mut topology_seed_refs = Vec::new();
        for target in query.scopes().into_iter().flat_map(|node| &node.targets) {
            if let QueryTarget::SchemaNeighborhood { schema } = target {
                let (bound_ref, _, _) = self
                    .prepared_reference(
                        query.subject,
                        &CognitiveRef::CognitiveSchema(*schema),
                        view,
                        bindings,
                    )
                    .await?;
                topology_seed_refs.push((bound_ref, "explicit_schema".into()));
            }
        }
        for cue in query.scopes().into_iter().flat_map(|node| &node.cues) {
            match cue {
                Cue::Schema(value) => {
                    let (bound_ref, _, _) = self
                        .prepared_reference(
                            query.subject,
                            &CognitiveRef::CognitiveSchema(value.schema),
                            view,
                            bindings,
                        )
                        .await?;
                    topology_seed_refs.push((bound_ref, "explicit_schema".into()));
                }
                Cue::Relation(value) => {
                    for reference in [&value.from, &value.to] {
                        let (bound_ref, _, _) = self
                            .prepared_reference(query.subject, reference, view, bindings)
                            .await?;
                        topology_seed_refs.push((bound_ref, "relation_cue".into()));
                    }
                }
                _ => {}
            }
        }

        Ok(topology_seed_refs)
    }
    pub async fn bind_query(&self, query: CognitiveQuery) -> Result<BoundQuery> {
        let snapshot = self.configuration.snapshot_for_subject(query.subject)?;
        self.bind_query_with_snapshot(query, snapshot).await
    }
    pub async fn bind_query_with_snapshot(
        &self,
        query: CognitiveQuery,
        config_snapshot: nous_configuration::ConfigSnapshot,
    ) -> Result<BoundQuery> {
        self.bind_query_with_authority_view(query, config_snapshot, None)
            .await
    }
    pub async fn bind_query_with_authority_view(
        &self,
        mut query: CognitiveQuery,
        config_snapshot: nous_configuration::ConfigSnapshot,
        historical_authority: Option<std::sync::Arc<HistoricalAuthoritySnapshot>>,
    ) -> Result<BoundQuery> {
        self.capture_query_clock(&mut query);
        super::historical_context::validate_view(&query, historical_authority.as_deref())?;
        let view = historical_authority.as_deref();
        super::closure::validate_query_input(&query)?;
        for node in query.scopes() {
            let mut scoped = query.clone();
            scoped.expression = node.clone();
            validate_hard_constraints(&scoped)?;
        }
        self.require_subject(query.subject).await?;
        let mut bindings = ReferenceBindings::new();
        self.canonicalize_query_tags(&mut query, view, &mut bindings)
            .await?;
        let retrieval_policy = resolve_retrieval_policy(&config_snapshot)?;
        if let Some(session) = query.session {
            self.require_session(query.subject, session).await?;
        }

        let context_snapshot = self.prepare_query_context(&mut query, view).await?;
        let mut representation_sources = context_snapshot.source_refs.clone();
        let context_degradation = context_snapshot.history_exclusions.clone();
        let (exact_bindings, mut allowed_revision_refs) =
            self.bind_exact_targets(&query, view, &mut bindings).await?;
        let (runtime_refs, runtime_sources) = self
            .bind_runtime_sources(query.subject, &representation_sources, view, &mut bindings)
            .await?;
        let mut topology_seed_refs = self
            .bind_structural_query_refs(&query, view, &mut bindings)
            .await?;
        topology_seed_refs.extend(
            runtime_sources
                .iter()
                .filter(|(_, s)| s == "work_context")
                .cloned(),
        );

        if let Some(view) = view {
            allowed_revision_refs.extend(
                view.cognition
                    .iter()
                    .flat_map(|state| state.revisions.iter().cloned()),
            );
        }
        let revision_policy = if allowed_revision_refs.is_empty() {
            RevisionPolicy::CurrentOnly
        } else {
            RevisionPolicy::ExactHistorical {
                allowed_revision_refs,
            }
        };
        let concept_enrichment = if query.is_exact_read() {
            super::ConceptEnrichment::Off
        } else {
            config_snapshot.get(super::CONCEPT_ENRICHMENT)?
        };
        let mut enabled_lanes = planned_profile_lanes(&query, retrieval_policy.cognitive_profile);
        include_concept_lane(
            &query,
            concept_enrichment,
            &runtime_refs,
            &mut enabled_lanes,
        );
        if enabled_lanes.is_empty() && !query.requests_resources() {
            return Err(Error::Invalid("query has no enabled retrieval lane".into()));
        }
        let lane_budgets = budget_values(&query, &enabled_lanes, &retrieval_policy);
        let bound_at_authority_seq = self.store.authority_seq(query.subject).await?;
        representation_sources.extend(
            exact_bindings
                .iter()
                .map(|binding| (binding.bound_ref.clone(), "explicit_exact".into())),
        );
        representation_sources.extend(
            runtime_refs
                .iter()
                .cloned()
                .map(|reference| (reference, "runtime_context".into())),
        );
        let representation = self
            .resolve_query_representation(
                &query,
                context_snapshot.work_context.clone(),
                representation_sources,
                &config_snapshot,
                view,
            )
            .await?;
        let mut activation = super::QueryActivation::prepared(
            &query,
            representation.sha256.clone(),
            exact_bindings.iter().map(|b| b.bound_ref.clone()).collect(),
            runtime_refs.clone(),
            &topology_seed_refs,
        );
        activation.degradation.extend(context_degradation);
        Ok(BoundQuery {
            context_snapshot: std::sync::Arc::new(context_snapshot),
            historical_authority,
            activation_view: None,
            activation,
            concept_enrichment,
            representation: std::sync::Arc::new(representation),
            query_id: Uuid::now_v7(),
            bound_at: self.now(query.subject),
            source_query: query.clone(),
            bound_at_authority_seq,
            revision_policy,
            exact_bindings,
            runtime_refs,
            runtime_sources,
            topology_seed_refs,
            enabled_lanes: enabled_lanes.clone(),
            lane_budgets,
            selected_embedding_space: None,
            topology_required: !query.is_exact_read()
                && explicit_topology(&query)
                && retrieval_policy.cognitive_profile.requirements().topology,
            config_snapshot,
            retrieval_policy,
        })
    }
}

impl BoundQuery {
    /// Research readout selection from the same closed query/context snapshot.
    pub fn for_profile(&self, profile: super::CognitiveProfile) -> Result<Self> {
        let mut bound = self.clone();
        bound.config_snapshot = self
            .config_snapshot
            .query_override(super::COGNITIVE_PROFILE, profile)?;
        bound.retrieval_policy = resolve_retrieval_policy(&bound.config_snapshot)?;
        bound.enabled_lanes = planned_profile_lanes(&bound.source_query, profile);
        include_concept_lane(
            &bound.source_query,
            bound.concept_enrichment,
            &bound.runtime_refs,
            &mut bound.enabled_lanes,
        );
        bound.lane_budgets = budget_values(
            &bound.source_query,
            &bound.enabled_lanes,
            &bound.retrieval_policy,
        );
        bound.topology_required = !bound.source_query.is_exact_read()
            && explicit_topology(&bound.source_query)
            && profile.requirements().topology;
        Ok(bound)
    }
}

fn visit_expression_tags(expression: &mut CognitiveQueryExpr, visit: &mut impl FnMut(&mut TagId)) {
    for cue in &mut expression.cues {
        visit_cue_tags(cue, visit);
    }
    for target in &mut expression.targets {
        if let QueryTarget::Exact { reference } = target {
            visit_ref_tag(reference, visit);
        }
    }
    for preference in &mut expression.preferences {
        match &mut preference.operand {
            PreferenceOperand::Cue(cue) => visit_cue_tags(cue, visit),
            PreferenceOperand::Exact(reference) => visit_ref_tag(reference, visit),
            PreferenceOperand::Recent(_) => {}
        }
    }
    for child in &mut expression.children {
        visit_expression_tags(child, visit);
    }
}
fn visit_cue_tags(cue: &mut Cue, visit: &mut impl FnMut(&mut TagId)) {
    match cue {
        Cue::Tag(value) => visit(&mut value.tag),
        Cue::Relation(value) => {
            visit_ref_tag(&mut value.from, visit);
            visit_ref_tag(&mut value.to, visit);
        }
        Cue::MediaRegion(value) => visit_ref_tag(&mut value.region, visit),
        _ => {}
    }
}
fn visit_ref_tag(reference: &mut CognitiveRef, visit: &mut impl FnMut(&mut TagId)) {
    if let CognitiveRef::Tag(tag) = reference {
        visit(tag);
    }
}
