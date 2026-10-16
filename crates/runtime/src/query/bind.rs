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
    if query.session.is_some()
        || query.work_context.is_some()
        || !query.situation.current_refs.is_empty()
    {
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
    sources: &[super::QuerySource],
    lanes: &mut Vec<EvidenceFamily>,
) {
    if !query.is_exact_read()
        && (enrichment != super::ConceptEnrichment::Off
            || sources
                .iter()
                .any(|source| matches!(source.reference, CognitiveRef::Tag(_))))
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
    pub async fn capture_query_clock(&self, query: &mut CognitiveQuery) -> Result<()> {
        if query.temporal_frame.authority_now == chrono::DateTime::<chrono::Utc>::UNIX_EPOCH {
            if query.temporal_frame.clock_now.is_none() {
                let clock = self.read_clock(query.subject).await?;
                query.temporal_frame.authority_now = clock.recorded_at;
                query.temporal_frame.clock_now = Some(clock);
            } else {
                query.temporal_frame.authority_now = self.recorded_now();
            }
        }
        let domain = query
            .temporal_frame
            .clock_now
            .as_ref()
            .ok_or_else(|| Error::Infrastructure("query clock was not captured".into()))?
            .time_domain_ref
            .clone();
        qualify_temporal_domains(&mut query.expression, &domain)?;
        Ok(())
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
            if let Some(read) = &query.source_read {
                let mut tx = self.store.read_view().await?;
                if !nous_persistence::AuthorityStore::source_read_allowed_in(
                    &mut tx, read, reference,
                )
                .await?
                {
                    return Err(Error::Unavailable(
                        "exact target unavailable under source scope".into(),
                    ));
                }
            }
            let (bound_ref, epoch, mutable_object) = self
                .prepared_reference(query.subject, reference, view, bindings)
                .await?;
            if let Some(read) = &query.source_read {
                let mut tx = self.store.read_view().await?;
                if !nous_persistence::AuthorityStore::source_read_allowed_in(
                    &mut tx, read, &bound_ref,
                )
                .await?
                {
                    return Err(Error::Unavailable("exact target source read denied (unknown/cyclic/over-budget lineage is withheld)".into()));
                }
            }
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
    async fn bind_sources(
        &self,
        subject: SubjectId,
        sources: &mut Vec<super::QuerySource>,
        view: Option<&HistoricalAuthoritySnapshot>,
        bindings: &mut ReferenceBindings,
    ) -> Result<()> {
        for source in sources.iter_mut() {
            source.reference = self
                .prepared_reference(subject, &source.requested_ref, view, bindings)
                .await?
                .0;
        }
        sources.sort_by_key(|source| {
            (
                source.reference.to_string(),
                source.origin.clone(),
                source.requested_ref.to_string(),
            )
        });
        sources.dedup_by(|a, b| a.reference == b.reference && a.origin == b.origin);
        Ok(())
    }
    pub async fn bind_query(
        &self,
        query: CognitiveQuery,
        resolver: &dyn crate::ContextResolver,
    ) -> Result<BoundQuery> {
        let snapshot = self.configuration.snapshot_for_subject(query.subject)?;
        self.bind_query_with_snapshot(query, snapshot, resolver)
            .await
    }
    pub async fn bind_query_with_snapshot(
        &self,
        query: CognitiveQuery,
        config_snapshot: nous_configuration::ConfigSnapshot,
        resolver: &dyn crate::ContextResolver,
    ) -> Result<BoundQuery> {
        self.bind_query_with_authority_view(query, config_snapshot, None, resolver)
            .await
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one binding owns source admission, canonical identity and frozen inputs"
    )]
    pub async fn bind_query_with_authority_view(
        &self,
        mut query: CognitiveQuery,
        config_snapshot: nous_configuration::ConfigSnapshot,
        historical_authority: Option<std::sync::Arc<HistoricalAuthoritySnapshot>>,
        resolver: &dyn crate::ContextResolver,
    ) -> Result<BoundQuery> {
        self.capture_query_clock(&mut query).await?;
        super::historical_context::validate_view(&query, historical_authority.as_deref())?;
        let view = historical_authority.as_deref();
        super::closure::validate_query_input(&query)?;
        for node in query.scopes() {
            let mut scoped = query.clone();
            scoped.expression = node.clone();
            validate_hard_constraints(&scoped)?;
        }
        self.require_subject(query.subject).await?;
        let authored_sources = expression_sources(&query);
        let mut bindings = ReferenceBindings::new();
        self.canonicalize_query_tags(&mut query, view, &mut bindings)
            .await?;
        let retrieval_policy = resolve_retrieval_policy(&config_snapshot)?;
        let mut context_snapshot = self.prepare_query_context(&mut query, view).await?;
        let (exact_bindings, mut allowed_revision_refs) =
            self.bind_exact_targets(&query, view, &mut bindings).await?;
        context_snapshot.source_refs.extend(authored_sources);
        self.bind_sources(
            query.subject,
            &mut context_snapshot.source_refs,
            view,
            &mut bindings,
        )
        .await?;
        if let Some(read) = &query.source_read {
            let mut tx = self.store.read_view().await?;
            let mut admitted = Vec::new();
            for source in context_snapshot.source_refs.drain(..) {
                if nous_persistence::AuthorityStore::source_read_allowed_in(
                    &mut tx,
                    read,
                    &source.reference,
                )
                .await?
                {
                    admitted.push(source);
                }
            }
            context_snapshot.source_refs = admitted;
            let admitted_refs = context_snapshot
                .source_refs
                .iter()
                .flat_map(|source| [source.requested_ref.clone(), source.reference.clone()])
                .collect::<HashSet<_>>();
            if let Some(session) = &mut context_snapshot.session {
                session
                    .resident
                    .retain(|resident| admitted_refs.contains(&resident.reference));
            }
            query.situation.current_refs.retain(|r| {
                context_snapshot
                    .source_refs
                    .iter()
                    .any(|s| &s.reference == r)
            });
        }
        // Captured owner views retain their object/revision semantics. Merged Tags have
        // only one canonical identity in the frozen input, including its raw anchors.
        let canonical_tag = |reference: &mut CognitiveRef| {
            if matches!(reference, CognitiveRef::Tag(_))
                && let Some((canonical, _, _)) = bindings.get(reference)
            {
                *reference = canonical.clone();
            }
        };
        for reference in &mut query.situation.current_refs {
            canonical_tag(reference);
        }
        if let Some(session) = &mut context_snapshot.session {
            for resident in &mut session.resident {
                canonical_tag(&mut resident.reference);
            }
        }
        if let Some(context) = &mut context_snapshot.work_context {
            for anchor in &mut context.cognition_anchors {
                canonical_tag(anchor);
            }
            for tag in &mut context.tag_anchors {
                let mut reference = CognitiveRef::Tag(*tag);
                canonical_tag(&mut reference);
                if let CognitiveRef::Tag(canonical) = reference {
                    *tag = canonical;
                }
            }
            context.tag_anchors.sort();
            context.tag_anchors.dedup();
        }
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
            &context_snapshot.source_refs,
            &mut enabled_lanes,
        );
        if enabled_lanes.is_empty() && !query.requests_resources() {
            return Err(Error::Invalid("query has no enabled retrieval lane".into()));
        }
        let lane_budgets = budget_values(&query, &enabled_lanes, &retrieval_policy);
        let bound_at_authority_seq = self.store.authority_seq(query.subject).await?;
        let representation = self
            .resolve_query_representation(
                &query,
                context_snapshot.work_context.clone(),
                context_snapshot.source_refs.clone(),
                &config_snapshot,
                view,
                resolver,
            )
            .await?;
        let mut activation = super::QueryActivation::prepared(
            &query,
            representation.sha256.clone(),
            exact_bindings.iter().map(|b| b.bound_ref.clone()).collect(),
            &context_snapshot.source_refs,
        );
        activation
            .degradation
            .extend(context_snapshot.history_exclusions.clone());
        Ok(BoundQuery {
            social_authorized_refs: Vec::new(),
            context_snapshot: std::sync::Arc::new(context_snapshot),
            historical_authority,
            activation_view: None,
            activation,
            concept_enrichment,
            representation: std::sync::Arc::new(representation),
            query_id: Uuid::now_v7(),
            bound_at: self.recorded_now(),
            source_query: query.clone(),
            bound_at_authority_seq,
            revision_policy,
            exact_bindings,
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
            &bound.context_snapshot.source_refs,
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

pub(super) fn expression_sources(query: &CognitiveQuery) -> Vec<super::QuerySource> {
    let mut sources = Vec::new();
    for scope in query.scopes() {
        for target in &scope.targets {
            let (reference, origin) = match target {
                QueryTarget::Exact { reference } => (reference.clone(), "exact_target"),
                QueryTarget::SchemaNeighborhood { schema } => {
                    (CognitiveRef::CognitiveSchema(*schema), "explicit_schema")
                }
                QueryTarget::EntityNeighborhood { entity_ref } => {
                    (CognitiveRef::Entity(entity_ref.clone()), "entity_cue")
                }
            };
            sources.push(super::QuerySource::new(reference, origin));
        }
        for cue in &scope.cues {
            let (reference, origin) = match cue {
                Cue::Entity(v) => (CognitiveRef::Entity(v.entity_ref.clone()), "entity_cue"),
                Cue::Tag(v) => (CognitiveRef::Tag(v.tag), "tag_cue"),
                Cue::Schema(v) => (CognitiveRef::CognitiveSchema(v.schema), "explicit_schema"),
                Cue::Resource(v) => (CognitiveRef::Resource(v.resource.clone()), "explicit_cue"),
                Cue::Object(v) => (
                    CognitiveRef::ExternalObject(v.object_ref.clone()),
                    "explicit_cue",
                ),
                Cue::Artifact(v) => (CognitiveRef::Artifact(v.artifact), "explicit_cue"),
                Cue::MediaRegion(v) => (v.region.clone(), "explicit_cue"),
                Cue::Relation(v) => {
                    sources.push(super::QuerySource::new(v.from.clone(), "relation_cue"));
                    (v.to.clone(), "relation_cue")
                }
                _ => continue,
            };
            sources.push(super::QuerySource::new(reference, origin));
        }
    }
    sources
}

fn visit_expression_tags(expression: &mut CognitiveQueryExpr, visit: &mut impl FnMut(&mut TagId)) {
    for cue in &mut expression.cues {
        visit_cue_tags(cue, visit);
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

fn qualify_temporal_domains(expression: &mut CognitiveQueryExpr, fallback: &str) -> Result<()> {
    let constraints = &mut expression.constraints;
    for (predicate, domain) in [
        (constraints.occurred, &mut constraints.time_domains.occurred),
        (constraints.observed, &mut constraints.time_domains.observed),
        (constraints.valid, &mut constraints.time_domains.valid),
        (constraints.formed, &mut constraints.time_domains.formed),
    ] {
        if predicate.is_some() {
            let value = domain.get_or_insert_with(|| fallback.into());
            if value.trim().is_empty() || value.len() > 256 {
                return Err(Error::Invalid("invalid predicate time domain".into()));
            }
        } else if domain.is_some() {
            return Err(Error::Invalid(
                "time domain requires its axis predicate".into(),
            ));
        }
    }
    for child in &mut expression.children {
        qualify_temporal_domains(child, fallback)?;
    }
    Ok(())
}
