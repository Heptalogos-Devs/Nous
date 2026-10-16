// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use sqlx::Row;
const CURRENT_FOCUS: &str = r#"
WITH current AS (
 SELECT 'memory_revision'::text kind,o.current_revision_id id,o.object_epoch epoch FROM memory_objects o WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL SELECT 'cognitive_schema_revision',o.current_revision_id,o.object_epoch FROM cognitive_schemas o WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL SELECT 'episode_revision',o.current_revision_id,o.object_epoch FROM episode_objects o WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 UNION ALL SELECT 'journal_revision',o.current_revision_id,o.object_epoch FROM journal_objects o WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
) SELECT kind,id FROM current JOIN unnest($2::text[],$3::uuid[]) requested(kind,id) USING(kind,id)
"#;
impl MemoryService {
    async fn current_concept_references(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<std::collections::HashSet<CognitiveRef>> {
        let (kinds, ids): (Vec<_>, Vec<_>) = references
            .iter()
            .map(|reference| {
                let (kind, value) = reference_parts(reference);
                Ok((
                    kind,
                    value.parse::<Uuid>().map_err(|_| {
                        Error::Invalid("concept focus must be exact cognition".into())
                    })?,
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();
        sqlx::query(CURRENT_FOCUS)
            .bind(subject.0)
            .bind(kinds)
            .bind(ids)
            .fetch_all(self.store.pool())
            .await
            .map_err(db)?
            .into_iter()
            .map(|row| {
                parse_reference(
                    &row.try_get::<String, _>("kind").map_err(db)?,
                    &row.try_get::<Uuid, _>("id").map_err(db)?.to_string(),
                )
            })
            .collect()
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one local plan joins focus, one-hop endpoints and typed catalogs"
    )]
    pub async fn plan_concepts(
        &self,
        subject: SubjectId,
        focus: CognitiveRef,
    ) -> Result<ConceptPlan> {
        let snapshot = self.configuration.snapshot_for_subject(subject)?;
        let policy = snapshot.get(CONCEPT_MAINTENANCE)?;
        let config_digest = snapshot.digest_for(&[CONCEPT_MAINTENANCE.path(), ACCRETION.path()])?;
        let sequence = self.store.authority_seq(subject).await?;
        let projected = self
            .concept_focus(subject, &focus, &resolve_accessibility_policy(&snapshot)?)
            .await?;
        let epoch = projected.epoch;
        let text = projected.text.clone();
        let source_basis = projected.basis.clone();
        let entities = projected.entities.clone();
        let mut basis = BTreeMap::new();
        basis.insert(
            "s0".into(),
            AssociationBasis::Revision(RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: focus.clone(),
                basis_role: BasisRole::Direct,
            })),
        );
        let mut basis_keys = std::collections::HashSet::new();
        for source_basis_entry in source_basis.into_iter().take(12) {
            if basis_keys.insert(source_basis_entry.canonical_key()) {
                basis.insert(
                    format!("s{}", basis.len()),
                    AssociationBasis::Revision(source_basis_entry),
                );
            }
        }
        let nearby = self
            .concept_aboutness_neighbors(subject, &focus, &entities, &text)
            .await?;
        let mut references = BTreeMap::from([("c0".into(), focus.clone())]);
        let neighborhood = self
            .association_neighborhood(subject, focus.clone(), 16, 1)
            .await?;
        let mut partial = neighborhood.truncated;
        let provenance = self
            .provenance_summary(
                subject,
                &basis
                    .values()
                    .filter_map(|basis| match basis {
                        AssociationBasis::Revision(revision) => Some(revision.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )
            .await?;
        let distinct_roots = provenance
            .roots
            .iter()
            .filter(|root| root.certainty == EvidenceRootCertainty::Known)
            .count();
        let focus_context = projected.context(&basis);
        let mut cognition = vec![
            serde_json::json!({"key":"c0","kind":reference_parts(&focus).0,"epoch":epoch,"text":text,"exactBasisKey":"s0","context":focus_context,"distinctRoots":distinct_roots}),
        ];
        let mut neighbors = neighborhood.nodes.clone();
        for reference in nearby {
            if !neighbors.contains(&reference) {
                neighbors.push(reference);
            }
        }
        let neighbors = neighbors
            .into_iter()
            .filter(|reference| reference != &focus)
            .take(8)
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::MemoryRevision(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                        | CognitiveRef::EpisodeRevision(_)
                        | CognitiveRef::JournalRevision(_)
                )
            })
            .collect::<Vec<_>>();
        let eligible = self.current_concept_references(subject, &neighbors).await?;
        let mut descriptors = self
            .query_descriptors(
                subject,
                &neighbors
                    .iter()
                    .filter(|reference| eligible.contains(*reference))
                    .map(|reference| (reference.clone(), true))
                    .collect::<Vec<_>>(),
                CognitiveEffort::Maximum,
                self.cognition.recorded_now(),
                &snapshot,
                None,
                None,
            )
            .await?
            .into_iter()
            .map(|descriptor| (descriptor.reference.clone(), descriptor))
            .collect::<std::collections::HashMap<_, _>>();
        for reference in &neighbors {
            if let Some(descriptor) = descriptors.remove(reference) {
                let key = format!("c{}", cognition.len());
                let basis_key = format!("s{}", basis.len());
                basis.insert(
                    basis_key.clone(),
                    AssociationBasis::Revision(RevisionBasis::CognitionDependency(
                        CognitionDependency {
                            epistemic_relation: None,
                            target_revision: reference.clone(),
                            basis_role: BasisRole::Contextual,
                        },
                    )),
                );
                references.insert(key.clone(), reference.clone());
                cognition.push(serde_json::json!({"key":key,"kind":reference_parts(reference).0,"text":descriptor.text.chars().take(512).collect::<String>(),"exactBasisKey":basis_key}));
            }
        }
        let mut entity_input = Vec::new();
        for descriptor in self
            .store
            .identity_descriptors(
                subject,
                &entities
                    .into_iter()
                    .map(CognitiveRef::Entity)
                    .collect::<Vec<_>>(),
                None,
            )
            .await?
        {
            let key = format!("e{}", entity_input.len());
            references.insert(key.clone(), descriptor.reference);
            entity_input.push(serde_json::json!({"key":key,"text":descriptor.text.chars().take(256).collect::<String>()}));
        }
        let query_feedback = nous_runtime::linked_query_feedback(
            &self.store,
            subject,
            &focus,
            self.cognition.recorded_now(),
        )
        .await?;
        let feedback_tags = query_feedback
            .iter()
            .flat_map(|feedback| {
                feedback
                    .signals
                    .explicit_tags
                    .iter()
                    .chain(&feedback.signals.inferred_tags)
            })
            .map(|activation| activation.tag)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut tags = self
            .concept_tag_candidates(
                subject,
                &focus,
                &text,
                policy.max_candidates,
                &feedback_tags,
            )
            .await?;
        for tag in &tags {
            references.insert(tag.key.clone(), CognitiveRef::Tag(tag.target.tag_id));
        }
        let mut associations = Vec::new();
        for association in neighborhood.associations {
            let lookup = |reference: &CognitiveRef| {
                references
                    .iter()
                    .find(|(_, r)| *r == reference)
                    .map(|(key, _)| key.clone())
            };
            if let (Some(from), Some(to)) = (lookup(&association.from), lookup(&association.to)) {
                associations.push(ConceptAssociation {
                    key: format!("a{}", associations.len()),
                    id: association.association_evidence_id,
                    from,
                    to,
                    relation: association.relation_kind,
                });
            } else {
                partial = true;
            }
        }
        for event in self
            .cognition
            .recent_use_events(
                subject,
                &focus,
                &[
                    "referenced",
                    "acted_on",
                    "result_supported",
                    "corrected",
                    "pinned",
                ],
                4,
            )
            .await?
        {
            basis.insert(
                format!("s{}", basis.len()),
                AssociationBasis::UseEvent(event),
            );
        }
        let accretion_policy = snapshot.get(ACCRETION)?;
        for tag in &mut tags {
            tag.accretion = self
                .accretion_signal(subject, &CognitiveRef::Tag(tag.target.tag_id))
                .await?;
        }
        let mut merge_candidates = Vec::new();
        let mut split_candidates = Vec::new();
        for (index, left) in tags.iter().enumerate() {
            if left.accretion.as_ref().is_some_and(|s| s.broad_center) {
                split_candidates.push(left.key.clone());
            }
            for right in tags.iter().skip(index + 1) {
                let (Some(a), Some(b)) = (&left.accretion, &right.accretion) else {
                    continue;
                };
                if a.partial || b.partial {
                    continue;
                }
                let a = a
                    .member_refs
                    .iter()
                    .collect::<std::collections::HashSet<_>>();
                let b = b
                    .member_refs
                    .iter()
                    .collect::<std::collections::HashSet<_>>();
                let union = a.union(&b).count();
                if union > 0 && a.intersection(&b).count() as f64 / union as f64 >= 0.8 {
                    merge_candidates.push((left.key.clone(), right.key.clone()));
                }
            }
        }
        let source_context = self.concept_source_text(subject, &basis).await?;
        let model_input = serde_json::json!({"focusKey":"c0","policy":policy,"partial":partial,"cognition":cognition,"entities":entity_input,
            "tags":tags.iter().map(|tag|serde_json::json!({"key":tag.key,"content":tag.content,"aliases":tag.aliases,"attached":tag.attached,"semanticSimilarity":tag.semantic_score,"accretion":tag.accretion.as_ref().map(|s|serde_json::json!({"distinctRoots":s.independent_roots,"currentMembers":s.attached_cognition,"episodeRecurrence":s.cross_episode_recurrence,"observedSpanSeconds":s.observed_span_seconds,"associationDegree":s.association_degree,"meaningfulUse":s.meaningful_use,"counterevidence":s.counterevidence,"coherence":s.semantic_coherence,"genericity":s.broad_center,"reviewPriority":s.review_priority(&accretion_policy),"partial":s.partial}))})).collect::<Vec<_>>(),
            "associations":associations.iter().map(|a|serde_json::json!({"key":a.key,"from":a.from,"to":a.to,"relation":a.relation})).collect::<Vec<_>>(),
            "basis":basis.iter().map(|(key,s)|serde_json::json!({"key":key,"kind":match s {AssociationBasis::Revision(RevisionBasis::CognitionDependency(_))=>"exact_cognition",AssociationBasis::Revision(_)=>"source_evidence",AssociationBasis::UseEvent(_)=>"meaningful_use"}})).collect::<Vec<_>>(),"queryFeedback":query_feedback,"queryFeedbackSemantics":"meaningful_use signal; not source evidence or mutation authorization","sourceContext":source_context,"mergeCandidates":merge_candidates,"splitCandidates":split_candidates});
        if self.store.authority_seq(subject).await? != sequence {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "concept planning snapshot changed",
            )
            .into());
        }
        Ok(ConceptPlan {
            subject,
            authority_seq: sequence,
            config_digest,
            policy,
            references,
            tags,
            associations,
            basis,
            model_input,
        })
    }
    async fn concept_aboutness_neighbors(
        &self,
        subject: SubjectId,
        focus: &CognitiveRef,
        entities: &[EntityRef],
        text: &str,
    ) -> Result<Vec<CognitiveRef>> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        let rows:Vec<Uuid>=sqlx::query_scalar("SELECT r.memory_revision_id FROM memory_objects o JOIN memory_revisions r ON r.memory_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND r.memory_revision_id::text<>$2 AND EXISTS(SELECT 1 FROM memory_revision_aboutness a WHERE a.memory_revision_id=r.memory_revision_id AND a.entity_ref=ANY($3::text[])) ORDER BY ts_rank_cd(to_tsvector('simple',r.representation_text),to_tsquery('simple',replace(plainto_tsquery('simple',$4)::text,' & ',' | '))) DESC,r.recorded_at DESC,r.memory_revision_id LIMIT 8")
            .bind(subject.0).bind(reference_parts(focus).1).bind(entities.iter().map(|entity|entity.as_str()).collect::<Vec<_>>()).bind(text).fetch_all(self.store.pool()).await.map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|id| CognitiveRef::MemoryRevision(MemoryRevisionId(id)))
            .collect())
    }
    async fn concept_tag_candidates(
        &self,
        subject: SubjectId,
        focus: &CognitiveRef,
        text: &str,
        limit: usize,
        feedback_tags: &[TagId],
    ) -> Result<Vec<ConceptTag>> {
        let (kind, value) = reference_parts(focus);
        let rows=sqlx::query("SELECT t.tag_id,t.current_revision_id,r.label,r.description,r.kind_hint,COALESCE(t.tag_id IN (SELECT canonical_tag($1,feedback_tag) FROM unnest($5::uuid[]) feedback_tag),false) feedback_selected,COALESCE(v.aliases,ARRAY[]::text[]) aliases, EXISTS(SELECT 1 FROM association_evidence a WHERE a.subject_id=$1 AND a.from_ref_kind=$2 AND a.from_ref=$3 AND a.to_ref_kind='tag' AND canonical_tag($1,CASE WHEN a.to_ref_kind='tag' THEN a.to_ref::uuid END)=t.tag_id AND a.relation_kind='tag_attachment' AND a.polarity='positive' AND a.revoked_at IS NULL) OR EXISTS(SELECT 1 FROM memory_revision_tags m WHERE $2='memory_revision' AND m.memory_revision_id::text=$3 AND canonical_tag($1,m.tag_id)=t.tag_id) attached, ts_rank_cd(to_tsvector('simple',r.label||' '||COALESCE(r.description,'')||' '||COALESCE(v.display_name,'')||' '||array_to_string(COALESCE(v.aliases,ARRAY[]::text[]),' ')),to_tsquery('simple',replace(plainto_tsquery('simple',$4)::text,' & ',' | ')))::float8 lexical_score FROM tags t JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id LEFT JOIN lexical_bindings b ON b.object_kind='tag' AND b.canonical_ref=t.tag_id::text LEFT JOIN lexical_visibility v ON v.lexical_ref=b.lexical_ref AND v.subject_id=t.subject_id WHERE t.subject_id=$1 AND t.status='active' ORDER BY feedback_selected DESC,attached DESC,lexical_score DESC,t.created_at DESC,t.tag_id LIMIT 32")
            .bind(subject.0).bind(kind).bind(value).bind(text).bind(feedback_tags.iter().map(|tag|tag.0).collect::<Vec<_>>()).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut selected = std::collections::HashSet::new();
        for row in &rows {
            if row.try_get::<bool, _>("feedback_selected").map_err(db)? {
                selected.insert(row.try_get::<Uuid, _>("tag_id").map_err(db)?);
            }
        }
        let mut candidates = rows
            .into_iter()
            .map(|row| {
                Ok(ConceptTag {
                    key: String::new(),
                    target: TagExpectation {
                        tag_id: TagId(row.try_get("tag_id").map_err(db)?),
                        expected_revision_id: row.try_get("current_revision_id").map_err(db)?,
                    },
                    content: TagContent {
                        label: row.try_get("label").map_err(db)?,
                        description: row.try_get("description").map_err(db)?,
                        kind_hint: row.try_get("kind_hint").map_err(db)?,
                    },
                    aliases: row
                        .try_get::<Vec<String>, _>("aliases")
                        .map_err(db)?
                        .into_iter()
                        .take(8)
                        .map(|alias| alias.chars().take(128).collect())
                        .collect(),
                    attached: row.try_get("attached").map_err(db)?,
                    lexical_score: row.try_get("lexical_score").map_err(db)?,
                    semantic_score: None,
                    accretion: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let texts = candidates
            .iter()
            .map(|tag| {
                nous_core::tag_semantic_representation(
                    &tag.content.label,
                    tag.content.description.as_deref(),
                    tag.content.kind_hint.as_deref(),
                )
                .map(|semantic| semantic.text)
            })
            .collect::<Result<Vec<_>>>()?;
        let scores = self
            .store
            .cached_semantic_scores(subject, text, &texts)
            .await?;
        for (candidate, score) in candidates.iter_mut().zip(scores) {
            candidate.semantic_score = score;
        }
        candidates.retain(|tag| {
            tag.attached
                || selected.contains(&tag.target.tag_id.0)
                || tag.lexical_score > 0.0
                || tag.semantic_score.is_some_and(|score| score >= 0.5)
        });
        candidates.sort_by(|a, b| {
            b.attached
                .cmp(&a.attached)
                .then_with(|| b.lexical_score.total_cmp(&a.lexical_score))
                .then_with(|| {
                    b.semantic_score
                        .unwrap_or(-1.0)
                        .total_cmp(&a.semantic_score.unwrap_or(-1.0))
                })
                .then_with(|| a.target.tag_id.0.cmp(&b.target.tag_id.0))
        });
        candidates.truncate(limit);
        for (index, tag) in candidates.iter_mut().enumerate() {
            tag.key = format!("t{index}");
            tag.content.label = tag.content.label.chars().take(256).collect();
            tag.content.description = tag
                .content
                .description
                .take()
                .map(|text| text.chars().take(512).collect());
        }
        Ok(candidates)
    }
}
