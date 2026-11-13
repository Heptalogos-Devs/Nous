//! Locate and calibrate original-text units across all selected groups.
use super::*;

impl Snapshot {
    pub(super) fn locate_groups(
        &self,
        text: &str,
        groups: &[Value],
        allowed: &BTreeSet<u64>,
        vector: Option<&[f32]>,
        options: &Options,
        cancel: &CancellationToken,
    ) -> Result<BTreeMap<u64, Vec<Value>>> {
        let bases = groups
            .iter()
            .map(|group| {
                Ok((
                    group["group_row"].as_u64().context("Group row missing")?,
                    group["rank_basis_row"]
                        .as_u64()
                        .context("Group rank basis missing")?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut grouped = BTreeMap::<u64, Vec<u64>>::new();
        for row in allowed {
            let group = self.inner.candidates.group_rows[*row as usize];
            if bases.contains_key(&group) && self.inner.candidates.local[*row as usize] {
                grouped.entry(group).or_default().push(*row);
            }
        }
        for (group, basis) in &bases {
            grouped.entry(*group).or_insert_with(|| vec![*basis]);
        }
        let rows = grouped.values().flatten().copied().collect::<Vec<_>>();
        let keys = rows
            .iter()
            .map(|row| self.inner.candidates.keys[*row as usize].clone())
            .collect::<BTreeSet<_>>();
        let mut lexical = BTreeMap::<String, (usize, f64)>::new();
        let lexical_enabled = options
            .localize_signals
            .iter()
            .any(|signal| signal == "lexical");
        if lexical_enabled && !text.trim().is_empty() {
            let page = self.inner.lexical.search_allowed(
                text,
                "snapshot",
                "snapshot",
                rows.len(),
                &keys,
            )?;
            for (rank, hit) in page.hits.into_iter().enumerate() {
                lexical.insert(hit.id, (rank + 1, f64::from(hit.score)));
            }
        }
        let mut contextual = BTreeMap::<String, f64>::new();
        if lexical_enabled {
            for fragment in &options.localization_context {
                if fragment.contribution <= 0.0 || fragment.text.trim().is_empty() {
                    continue;
                }
                let page = self.inner.lexical.search_allowed(
                    &fragment.text,
                    "snapshot",
                    "snapshot",
                    rows.len(),
                    &keys,
                )?;
                for (rank, hit) in page.hits.into_iter().enumerate() {
                    *contextual.entry(hit.id).or_default() +=
                        fragment.contribution / (options.localize_rrf_k + rank as f64 + 1.0);
                }
                cancel.check()?;
            }
        }
        let mut semantic = Vec::<(u64, f64)>::new();
        let semantic_enabled = options
            .localize_signals
            .iter()
            .any(|signal| signal == "semantic");
        if let (true, Some(query), Some(vectors)) = (
            semantic_enabled,
            vector,
            self.inner.candidate_units.as_ref(),
        ) {
            for row in &rows {
                cancel.check()?;
                let score = vectors[*row as usize]
                    .iter()
                    .zip(query)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum();
                semantic.push((*row, score));
            }
            semantic.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        }
        let semantic = semantic
            .into_iter()
            .enumerate()
            .map(|(rank, (row, score))| (row, (rank + 1, score)))
            .collect::<BTreeMap<_, _>>();
        let lexical_weight = if lexical_enabled {
            options.localize_lexical_weight
        } else {
            0.0
        };
        let semantic_weight = if semantic_enabled {
            options.localize_semantic_weight
        } else {
            0.0
        };
        let query_weight = lexical_weight + semantic_weight;
        let mut scored = rows
            .into_iter()
            .map(|row| {
                let key = &self.inner.candidates.keys[row as usize];
                let lex = lexical.get(key);
                let sem = semantic.get(&row);
                let context = contextual.get(key).copied().unwrap_or(0.0);
                let score = if query_weight > 0.0 {
                    (lex.map_or(0.0, |(rank, _)| {
                        lexical_weight / (options.localize_rrf_k + *rank as f64)
                    }) + sem.map_or(0.0, |(rank, _)| {
                        semantic_weight / (options.localize_rrf_k + *rank as f64)
                    })) / query_weight
                } else {
                    0.0
                } + context;
                (row, score, lex.copied(), sem.copied(), context)
            })
            .collect::<Vec<_>>();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let reference = if query_weight > 0.0 {
            1.0 / (options.localize_rrf_k + 1.0)
        } else {
            0.0
        };
        let mut result = BTreeMap::<u64, Vec<Value>>::new();
        for (row, score, lexical_score, semantic_score, context_score) in scored {
            cancel.check()?;
            let index = row as usize;
            let group = self.inner.candidates.group_rows[index];
            let parts = result.entry(group).or_default();
            if parts.len() >= options.local_units_per_group {
                continue;
            }
            let evidence = self.inner.lexical.evidence(
                text,
                &self.inner.input.candidates.texts[index],
                &self.inner.candidates.titles[index],
            );
            parts.push(
                json!({"row":row,"reason":"local_score","lexical_evidence":evidence,
                "evidence_row":if row == bases[&group] {Some(row)}else{None},
                "scores":{"local_rrf":score,"rrf_reference":reference,
                    "local_relevance":if reference>0.0 {score/reference}else{0.0},
                    "lexical":lexical_score.map(|(_,value)|value),
                    "semantic":semantic_score.map(|(_,value)|value),
                    "lexical_rank":lexical_score.map(|(rank,_)|rank),
                    "semantic_rank":semantic_score.map(|(rank,_)|rank),
                    "lexical_weight":if lexical_enabled {Some(lexical_weight)}else{None},
                    "semantic_weight":if semantic_enabled {Some(semantic_weight)}else{None},
                    "context_lexical":if context_score>0.0 {Some(context_score)}else{None}}}),
            );
        }
        Ok(result)
    }
}
