//! Rebuildable long-term aggregation signals, never an Authority strength or truth field.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_configuration::*;
use std::collections::BTreeSet;
pub const ACCRETION: ConfigKey<AccretionPolicy> = ConfigKey::new("maintenance.accretion");
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AccretionPolicy {
    pub enabled: bool,
    pub generic_degree: usize,
    pub recurrence_review: usize,
    pub member_budget: usize,
    pub coherence_threshold: f64,
}
impl Default for AccretionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            generic_degree: 32,
            recurrence_review: 3,
            member_budget: 32,
            coherence_threshold: 0.35,
        }
    }
}
pub(super) fn register_accretion_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        ACCRETION,
        "memory",
        "Derived cognitive accretion catalog and maintenance priority limits.",
        AccretionPolicy::default(),
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |p| {
            if !(4..=256).contains(&p.generic_degree)
                || !(1..=16).contains(&p.recurrence_review)
                || !(1..=256).contains(&p.member_budget)
                || !p.coherence_threshold.is_finite()
                || !(0.0..=1.0).contains(&p.coherence_threshold)
            {
                return Err(Error::Invalid("invalid accretion policy".into()));
            }
            Ok(())
        },
    )
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccretionSignal {
    pub independent_roots: usize,
    pub attached_cognition: usize,
    pub cross_episode_recurrence: usize,
    pub observed_span_seconds: i64,
    pub association_degree: usize,
    pub relation_diversity: usize,
    pub meaningful_use: u64,
    pub counterevidence: u64,
    pub semantic_coherence: Option<f64>,
    pub broad_center: bool,
    pub partial: bool,
    pub member_refs: Vec<CognitiveRef>,
}
impl MemoryService {
    #[expect(
        clippy::too_many_lines,
        reason = "one signal combines direct support, recursive roots and owner usage facts"
    )]
    pub async fn accretion_signal(
        &self,
        subject: SubjectId,
        center: &CognitiveRef,
    ) -> Result<Option<AccretionSignal>> {
        let snapshot = self.configuration.snapshot_for_subject(subject)?;
        let policy = snapshot.get(ACCRETION)?;
        if !policy.enabled {
            return Ok(None);
        }
        self.store.validate_reference(subject, center).await?;
        let mut tx = self.store.read_view().await?;
        let mut members = self
            .accretion_members(&mut tx, subject, center, policy.member_budget)
            .await?;
        let total = members.1;
        let partial = total > members.0.len();
        let refs = std::mem::take(&mut members.0);
        let mut roots = BTreeSet::new();
        let mut occurrences = BTreeSet::new();
        let mut counterevidence = 0u64;
        let mut supports = self
            .exact_supports_in(&mut tx, subject, &refs, None)
            .await?;
        let contents = self
            .content_in(
                &mut tx,
                subject,
                &refs.iter().map(|r| (r.clone(), true)).collect::<Vec<_>>(),
                &resolve_accessibility_policy(&snapshot)?,
                None,
                self.cognition.recorded_now(),
                CognitiveEffort::Maximum,
                2048,
                None,
            )
            .await?;
        let direct_basis = refs
            .iter()
            .flat_map(|r| supports[r].basis.iter().cloned())
            .collect::<Vec<_>>();
        let texts = contents
            .into_iter()
            .filter_map(|c| match c {
                Ok(c) => Some(Ok(c.descriptor().text)),
                Err(Error::Unavailable(_) | Error::NotFound(_)) => None,
                Err(e) => Some(Err(e)),
            })
            .collect::<Result<Vec<_>>>()?;
        let summary = self
            .provenance_summary_in(&mut tx, subject, &direct_basis, None, &mut supports)
            .await?;
        tx.commit().await.map_err(db)?;
        roots.extend(
            summary
                .roots
                .into_iter()
                .filter(|r| r.certainty == EvidenceRootCertainty::Known)
                .map(|r| r.root_key),
        );
        for basis in direct_basis {
            match basis {
                RevisionBasis::Evidence(e) => {
                    occurrences.insert(e.occurrence_id.0);
                    counterevidence +=
                        u64::from(e.epistemic_relation == Some(EpistemicRelation::Contradicts));
                }
                RevisionBasis::CognitionDependency(d) => {
                    counterevidence +=
                        u64::from(d.epistemic_relation == Some(EpistemicRelation::Contradicts))
                }
                RevisionBasis::Seed(_) => {}
            }
        }
        let (kinds, values): (Vec<_>, Vec<_>) = refs.iter().map(reference_parts).unzip();
        let mut meaningful = 0u64;
        for fact in self
            .cognition
            .use_summaries(subject, &refs)
            .await?
            .into_values()
            .flatten()
        {
            if matches!(
                fact.kind.as_str(),
                "referenced" | "acted_on" | "result_supported" | "corrected" | "pinned"
            ) {
                meaningful += fact.count;
            }
            if fact.kind == "result_refuted" {
                counterevidence += fact.count;
            }
        }
        let episodes:i64=sqlx::query_scalar("WITH members AS (SELECT * FROM unnest($2::text[],$3::text[]) AS m(kind,value)) SELECT count(DISTINCT r.episode_revision_id)::bigint FROM episode_revisions r JOIN episode_objects o ON o.current_revision_id=r.episode_revision_id JOIN episode_revision_members e USING(episode_revision_id) WHERE r.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND (EXISTS(SELECT 1 FROM members m WHERE m.kind=e.ref_kind AND m.value=e.ref_value) OR (e.ref_kind='occurrence' AND e.ref_value=ANY($4::text[])))")
                .bind(subject.0).bind(&kinds).bind(&values).bind(occurrences.iter().map(ToString::to_string).collect::<Vec<_>>()).fetch_one(self.store.pool()).await.map_err(db)?;
        let span = self
            .material
            .occurrence_observed_span(
                subject,
                &occurrences
                    .iter()
                    .map(|id| OccurrenceId(*id))
                    .collect::<Vec<_>>(),
            )
            .await?;
        let (kind, value) = reference_parts(center);
        let degree=sqlx::query("SELECT count(*)::bigint degree,count(DISTINCT relation_kind)::bigint diversity,count(*) FILTER(WHERE polarity='negative')::bigint negative FROM association_evidence a WHERE a.subject_id=$1 AND a.revoked_at IS NULL AND ((a.from_ref_kind=$2 AND a.from_ref=$3) OR (a.to_ref_kind=$2 AND a.to_ref=$3) OR ($2='tag' AND ((a.from_ref_kind='tag' AND canonical_tag($1,CASE WHEN a.from_ref_kind='tag' THEN a.from_ref::uuid END)=$4) OR (a.to_ref_kind='tag' AND canonical_tag($1,CASE WHEN a.to_ref_kind='tag' THEN a.to_ref::uuid END)=$4))))")
                .bind(subject.0).bind(kind).bind(value).bind(if let CognitiveRef::Tag(id)=center {Some(id.0)}else {None}).fetch_one(self.store.pool()).await.map_err(db)?;
        let degree_count = usize::try_from(degree.try_get::<i64, _>("degree").map_err(db)?)
            .map_err(|_| Error::Infrastructure("invalid degree".into()))?;
        counterevidence += u64::try_from(degree.try_get::<i64, _>("negative").map_err(db)?)
            .map_err(|_| Error::Infrastructure("invalid negative count".into()))?;
        let coherence = if let Some(first) = texts.first() {
            let scores = self
                .store
                .cached_semantic_scores(subject, first, &texts)
                .await?;
            let values = scores.into_iter().skip(1).flatten().collect::<Vec<_>>();
            if values.is_empty() {
                None
            } else {
                Some(values.iter().sum::<f64>() / values.len() as f64)
            }
        } else {
            None
        };
        let broad = degree_count.max(total) > policy.generic_degree
            || coherence.is_some_and(|c| c < policy.coherence_threshold);
        Ok(Some(AccretionSignal {
            independent_roots: roots.len(),
            attached_cognition: total,
            cross_episode_recurrence: usize::try_from(episodes).unwrap_or(0),
            observed_span_seconds: span,
            association_degree: degree_count,
            relation_diversity: usize::try_from(degree.try_get::<i64, _>("diversity").map_err(db)?)
                .unwrap_or(0),
            meaningful_use: meaningful,
            counterevidence,
            semantic_coherence: coherence,
            broad_center: broad,
            partial,
            member_refs: refs,
        }))
    }
    async fn accretion_members(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        center: &CognitiveRef,
        limit: usize,
    ) -> Result<(Vec<CognitiveRef>, usize)> {
        let members = match center {
            CognitiveRef::Tag(_) => {
                "SELECT 'memory_revision'::text kind,m.memory_revision_id::text value FROM memory_revision_tags m JOIN memory_revisions r USING(memory_revision_id) WHERE r.subject_id=$1 AND canonical_tag($1,m.tag_id)=canonical_tag($1,$2) UNION SELECT a.from_ref_kind,a.from_ref FROM association_evidence a WHERE a.subject_id=$1 AND a.to_ref_kind='tag' AND canonical_tag($1,CASE WHEN a.to_ref_kind='tag' THEN a.to_ref::uuid END)=canonical_tag($1,$2) AND a.relation_kind='tag_attachment' AND a.polarity='positive' AND a.revoked_at IS NULL"
            }
            CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_) => {
                "SELECT basis_kind kind,basis_ref value FROM cognitive_schema_evidence_links l JOIN cognitive_schema_revisions r USING(schema_revision_id) JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 AND l.revoked_at IS NULL AND basis_kind IN ('memory_revision','cognitive_schema_revision','episode_revision','journal_revision')"
            }
            CognitiveRef::MemoryRevision(_) => return Ok((vec![center.clone()], 1)),
            _ => {
                return Err(Error::Invalid(
                    "accretion center must be Tag, Schema or exact Memory".into(),
                ));
            }
        };
        let live = "SELECT 'memory_revision'::text kind,current_revision_id::text value FROM memory_objects WHERE subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' UNION SELECT 'cognitive_schema_revision',current_revision_id::text FROM cognitive_schemas WHERE subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' UNION SELECT 'episode_revision',current_revision_id::text FROM episode_objects WHERE subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' UNION SELECT 'journal_revision',current_revision_id::text FROM journal_objects WHERE subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal'";
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("WITH live AS (");
        query.push(live).push("),members AS (").push(members).push("),eligible AS(SELECT DISTINCT kind,value FROM members JOIN live USING(kind,value)) SELECT *,count(*) OVER() total FROM eligible ORDER BY kind,value LIMIT $3");
        let id = match center {
            CognitiveRef::Tag(id) => id.0,
            CognitiveRef::CognitiveSchemaRevision(id) => id.0,
            CognitiveRef::CognitiveSchema(_) => {
                sqlx::query_scalar::<_,Uuid>("SELECT current_revision_id FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2")
                    .bind(subject.0).bind(center.uuid()).fetch_optional(&mut **tx).await.map_err(db)?
                    .ok_or_else(|| Error::NotFound("accretion Schema not found".into()))?
            }
            _ => unreachable!(),
        };
        let rows = query
            .build()
            .bind(subject.0)
            .bind(id)
            .bind(
                i64::try_from(limit)
                    .map_err(|_| Error::Invalid("member budget exceeded".into()))?,
            )
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
        let total = rows
            .first()
            .map(|r| r.try_get::<i64, _>("total").map_err(db))
            .transpose()?
            .unwrap_or(0);
        let refs = rows
            .into_iter()
            .map(|r| {
                parse_reference(
                    &r.try_get::<String, _>("kind").map_err(db)?,
                    &r.try_get::<String, _>("value").map_err(db)?,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((
            refs,
            usize::try_from(total)
                .map_err(|_| Error::Infrastructure("invalid member count".into()))?,
        ))
    }
}

impl AccretionSignal {
    pub fn review_priority(&self, policy: &AccretionPolicy) -> u32 {
        if self.broad_center {
            40
        } else if self.cross_episode_recurrence >= policy.recurrence_review
            || self.meaningful_use > 0
        {
            30
        } else {
            20
        }
    }
}
