//! Per-query authority restrictions, applied before candidate offering and graph traversal.
use super::*;
pub(super) struct AuthorityView {
    pub(super) assets: MemoAssets,
    pub(super) geometry_available: bool,
}
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
    ) -> Result<Option<AuthorityView>> {
        let full_candidates = allowed.is_none_or(|ids| ids.len() == self.mappings.candidates.len());
        let full_tags = allowed_tags.is_none_or(|ids| ids.len() == self.mappings.tags.len());
        if full_candidates && full_tags {
            return Ok(None);
        }
        let mut input = self.assets.snapshot();
        input
            .candidates
            .retain(|id, _| allowed.is_none_or(|ids| ids.contains(id)));
        let roots = input
            .candidates
            .values()
            .flat_map(|candidate| candidate.source_ids.iter().copied())
            .collect::<std::collections::HashSet<_>>();
        let files = input
            .candidates
            .values()
            .map(|candidate| candidate.file_id)
            .chain(roots.iter().copied())
            .collect::<BTreeSet<_>>();
        input.files.retain(|id, _| files.contains(id));
        input
            .tag_names
            .retain(|id, _| allowed_tags.is_none_or(|ids| ids.contains(id)));
        let tags = input
            .tag_names
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        input.tag_vectors.retain(|id, _| tags.contains(id));
        for file in input.files.values_mut() {
            file.tags.retain(|row| tags.contains(&row.tag_id));
        }
        let source_scope_changed = input
            .graph
            .provenance
            .values()
            .flatten()
            .any(|(root, _)| !roots.contains(root));
        if !full_tags || source_scope_changed {
            input.graph = Arc::new(
                input
                    .graph
                    .authorized_view(&tags, &roots)
                    .map_err(anyhow::Error::msg)?,
            );
            input.epa = None;
        }
        let assets = MemoAssets::try_new(input).map_err(anyhow::Error::msg)?;
        Ok(Some(AuthorityView {
            assets,
            geometry_available: full_tags && !source_scope_changed,
        }))
    }
}
