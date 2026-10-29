//! Per-query authority restrictions, applied before candidate offering and graph traversal.
use super::*;
impl ServingGeneration {
    pub fn search_authorized(
        &self,
        query: &[f32],
        limit: usize,
        profile: ServingProfile,
        scope: ServingQueryScope<'_>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ServingResult> {
        ensure!(!cancelled(), "CANCELLED: query cancelled");
        let allowed = self
            .mappings
            .candidates
            .iter()
            .filter(|(_, item)| scope.references.contains(&item.id))
            .map(|(id, _)| *id)
            .collect::<BTreeSet<_>>();
        ensure!(
            allowed.len() == scope.references.len(),
            "candidate filter contains unknown reference"
        );
        let allowed_tags = scope.tag_keys.map(|keys| {
            self.mappings
                .tags
                .iter()
                .filter(|(_, key)| keys.contains(*key))
                .map(|(id, _)| *id)
                .collect::<BTreeSet<_>>()
        });
        ensure!(
            scope
                .tag_keys
                .zip(allowed_tags.as_ref())
                .is_none_or(|(keys, ids)| keys.len() == ids.len()),
            "graph filter contains unknown tag"
        );
        let row_by_reference = self
            .mappings
            .candidates
            .iter()
            .filter(|(id, _)| allowed.contains(*id))
            .map(|(id, item)| (item.id.as_str(), *id))
            .collect::<BTreeMap<_, _>>();
        let mut lexical = BTreeMap::new();
        for hit in scope.lexical {
            ensure!(
                hit.score.is_finite() && hit.score >= 0.0,
                "invalid lexical score"
            );
            let row = row_by_reference
                .get(hit.reference.as_str())
                .context("lexical candidate is outside authorized frozen mapping")?;
            ensure!(
                lexical.insert(*row, hit.score).is_none(),
                "duplicate lexical candidate"
            );
        }
        match profile {
            ServingProfile::Dense => {
                let hits = self.ann.search_candidates(query, limit, Some(&allowed))?;
                let source_mappings = self.sources_for(hits.iter().map(|hit| hit.id))?;
                Ok(ServingResult {
                    ranked: hits
                        .into_iter()
                        .map(|hit| self.materialize(hit.id, hit.similarity, None))
                        .collect::<Result<_>>()?,
                    generation: self.identity.generation.clone(),
                    space: self.identity.space.clone(),
                    source_mappings,
                    diagnostics: json!({"profile":"dense","engine":"usearch","allowed_candidates":allowed.len()}),
                })
            }
            ServingProfile::Vcp(config) => self.vcp_search(
                query,
                limit,
                *config,
                ScopedVcpCandidates {
                    allowed: Some(&allowed),
                    tags: allowed_tags.as_ref(),
                    lexical: &lexical,
                },
                Some(cancelled),
            ),
        }
    }
    pub(super) fn restrict_assets(
        &self,
        allowed: Option<&BTreeSet<i64>>,
        allowed_tags: Option<&BTreeSet<i64>>,
    ) -> Result<Option<MemoAssets>> {
        let full_candidates = allowed.is_none_or(|ids| ids.len() == self.mappings.candidates.len());
        let full_tags = allowed_tags.is_none_or(|ids| ids.len() == self.mappings.tags.len());
        if full_candidates && full_tags {
            return Ok(None);
        }
        let mut prepared = self.prepared.clone();
        if let Some(ids) = allowed {
            prepared
                .mappings
                .candidates
                .retain(|id, _| ids.contains(id));
        }
        let roots = prepared
            .mappings
            .candidates
            .values()
            .flat_map(|candidate| candidate.source_refs.iter().cloned())
            .collect::<BTreeSet<_>>();
        if prepared.snapshot.graph.is_none() {
            prepared
                .mappings
                .roots
                .retain(|_, root| roots.contains(root));
            prepared.root_tags.retain(|root, _| roots.contains(root));
        }
        let file_ids = prepared
            .mappings
            .candidates
            .values()
            .map(|item| item.file_id)
            .chain(prepared.mappings.roots.keys().copied())
            .collect::<BTreeSet<_>>();
        prepared
            .mappings
            .source_sets
            .retain(|id, _| file_ids.contains(id));
        let permitted_keys = prepared
            .root_tags
            .values()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        prepared.mappings.tags.retain(|id, key| {
            (prepared.snapshot.graph.is_some() || permitted_keys.contains(key))
                && allowed_tags.is_none_or(|ids| ids.contains(id))
        });
        let tag_keys = prepared
            .mappings
            .tags
            .values()
            .cloned()
            .collect::<BTreeSet<_>>();
        let tag_ids = prepared
            .mappings
            .tags
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        prepared
            .tag_definitions
            .retain(|key, _| tag_keys.contains(key));
        prepared.unit_tags.retain(|id, _| tag_ids.contains(id));
        for tags in prepared.root_tags.values_mut() {
            tags.retain(|key| tag_keys.contains(key));
        }
        // Reuse the original geometry, transport, provenance and EPA builders on
        // the selected evidence roots while retaining stable candidate/Tag IDs.
        let (assets, _, _, _) = build_assets(&prepared)?;
        Ok(Some(assets))
    }
}
