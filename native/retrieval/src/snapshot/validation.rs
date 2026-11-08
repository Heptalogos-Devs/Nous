//! Validate frozen Native snapshot inputs and their cross-references.
use super::*;

fn unique(values: &[String]) -> bool {
    values.iter().all(|s| !s.trim().is_empty())
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
impl Input {
    pub(super) fn validate(
        &self,
    ) -> Result<(CandidateMeta, ConceptMeta, FactMeta, AssociationMeta)> {
        let native_options = self.options.get("native").unwrap_or(&self.options);
        let build_config: nous_vcp::config::BuildConfig =
            serde_json::from_value(native_options.clone())?;
        build_config
            .validate()
            .map_err(|e| failure("wave.invalid_input", e))?;
        let c: CandidateMeta = serde_json::from_str(&self.candidates.metadata_json)?;
        let t: ConceptMeta = serde_json::from_str(&self.concepts.metadata_json)?;
        let f: FactMeta = serde_json::from_str(&self.facts.metadata_json)?;
        let a: AssociationMeta = serde_json::from_str(&self.associations.metadata_json)?;
        for name in ["snapshot_id", "space_signature", "build_config_signature"] {
            ensure!(
                self.identity
                    .get(name)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty()),
                "Missing snapshot identity {name}"
            );
        }
        let n = c.keys.len();
        let m = t.keys.len();
        let roots = f.root_keys.len();
        let seq = f.sequence_keys.len();
        let edges = a.keys.len();
        ensure!(
            unique(&c.keys)
                && unique(&t.keys)
                && unique(&f.root_keys)
                && unique(&f.sequence_keys)
                && unique(&a.keys),
            "Keys must be unique and nonempty"
        );
        ensure!(
            c.group_rows.len() == n
                && unique(&c.group_keys)
                && c.group_rows.iter().all(|r| *r < c.group_keys.len() as u64)
                && c.documents.len() == n
                && c.titles.len() == n
                && self.candidates.texts.len() == n
                && c.documents.iter().all(|d| !d.is_empty()),
            "Candidate metadata/text length mismatch"
        );
        let mut group_documents = BTreeMap::new();
        for (row, document) in c.documents.iter().enumerate() {
            let owner = group_documents.entry(c.group_rows[row]).or_insert(document);
            ensure!(
                *owner == document,
                "A candidate group must belong to one exact document"
            );
        }
        ensure!(
            group_documents.len() == c.group_keys.len(),
            "Group table contains unused rows"
        );
        ensure!(
            t.labels.len() == m
                && (t.aliases.is_empty() || t.aliases.len() == m)
                && f.root_checksums.len() == roots,
            "Concept or source metadata length mismatch"
        );
        csr(
            &self.candidates.membership_indptr,
            &self.candidates.membership_indices,
            n,
            m,
        )?;
        let paths = self
            .candidates
            .path_step_indptr
            .len()
            .checked_sub(1)
            .context("Missing path offsets")?;
        csr(
            &self.candidates.path_indptr,
            &(0..paths as u64).collect::<Vec<_>>(),
            n,
            paths,
        )?;
        csr(
            &self.candidates.path_step_indptr,
            &self.candidates.path_indices,
            paths,
            m,
        )?;
        ensure!(
            self.candidates
                .path_indptr
                .windows(2)
                .all(|p| p[1] - p[0] <= 16),
            "At most 16 paths per candidate"
        );
        csr(
            &self.candidates.root_indptr,
            &self.candidates.root_indices,
            n,
            roots,
        )?;
        ensure!(
            self.candidates.path_positions.len() == self.candidates.path_indices.len()
                && self.candidates.path_positions.iter().all(|p| *p > 0),
            "Ordered candidate curve positions must be positive"
        );
        csr(&self.facts.indptr, &self.facts.indices, seq, m)?;
        ensure!(
            self.facts.positions.len() == self.facts.indices.len()
                && self.facts.ordered.len() == seq
                && self.facts.sequence_roots.len() == seq
                && self.facts.sequence_roots.iter().all(|r| *r < roots as u64),
            "Source sequence shape mismatch"
        );
        for row in 0..seq {
            let positions = &self.facts.positions
                [self.facts.indptr[row] as usize..self.facts.indptr[row + 1] as usize];
            if self.facts.ordered[row] {
                ensure!(
                    positions.iter().all(|p| *p > 0) && positions.windows(2).all(|p| p[0] <= p[1]),
                    "Ordered source positions must be positive and monotone"
                );
            }
        }
        if let Some(limit) = build_config.pair_work_limit {
            let mut total = 0u128;
            for (i, key) in f.sequence_keys.iter().enumerate() {
                let n = (self.facts.indptr[i + 1] - self.facts.indptr[i]) as u128;
                let work = n.saturating_mul(n.saturating_sub(1)) / 2;
                total = total.saturating_add(work);
                if total > u128::from(limit) {
                    return Err(crate::Failure{code:"wave.pair_work_limit",message:format!("Occurrence pair work exceeds {limit} at source sequence {key}"),details:json!({"sequence":key,"sequence_pairs":work.to_string(),"total_pairs":total.to_string(),"limit":limit})}.into());
                }
            }
        }
        if let Some(v) = &self.candidates.vectors {
            ensure!(v.rows == n, "Candidate matrix rows mismatch");
        }
        if let Some(v) = &self.concepts.vectors {
            ensure!(v.rows == m, "Concept matrix rows mismatch");
            if let Some(c) = &self.candidates.vectors {
                ensure!(c.cols == v.cols, "Vector space dimensions mismatch");
            }
        }
        for matrix in [&self.candidates.vectors, &self.concepts.vectors]
            .into_iter()
            .flatten()
        {
            ensure!(
                matrix.cols > 0
                    && matrix.rows.checked_mul(matrix.cols) == Some(matrix.values.len())
                    && matrix.values.iter().all(|v| v.is_finite()),
                "Invalid finite matrix layout"
            );
        }
        for row in 0..n {
            let members = slice(
                &self.candidates.membership_indptr,
                &self.candidates.membership_indices,
                row,
            );
            for path in self.candidates.path_indptr[row] as usize
                ..self.candidates.path_indptr[row + 1] as usize
            {
                let indices = slice(
                    &self.candidates.path_step_indptr,
                    &self.candidates.path_indices,
                    path,
                );
                let positions = slice(
                    &self.candidates.path_step_indptr,
                    &self.candidates.path_positions,
                    path,
                );
                ensure!(
                    indices.iter().all(|i| members.contains(i))
                        && positions.windows(2).all(|p| p[0] <= p[1]),
                    "Path occurrences must have memberships and monotone positions"
                );
            }
        }
        let assoc = &self.associations;
        ensure!(
            a.relations.len() == edges
                && assoc.edge_groups.len() == edges
                && assoc
                    .edge_groups
                    .iter()
                    .all(|i| *i < a.support_groups.len() as u64)
                && assoc.sources.len() == edges
                && assoc.targets.len() == edges
                && assoc.directed.len() == edges
                && assoc.strengths.len() == edges
                && assoc.confidences.len() == edges
                && assoc.polarities.len() == edges,
            "Association shape mismatch"
        );
        ensure!(
            assoc
                .sources
                .iter()
                .chain(assoc.targets.iter())
                .all(|i| *i < m as u64)
                && assoc
                    .strengths
                    .iter()
                    .chain(assoc.confidences.iter())
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                && assoc.polarities.iter().all(|v| [-1, 1].contains(v)),
            "Association endpoint or quality invalid"
        );
        csr(
            &assoc.group_root_indptr,
            &assoc.group_root_indices,
            a.support_groups.len(),
            roots,
        )?;
        ensure!(
            unique(
                &a.support_groups
                    .iter()
                    .map(|g| g.key.clone())
                    .collect::<Vec<_>>()
            ) && a.support_groups.iter().enumerate().all(|(i, g)| matches!(
                g.kind.as_str(),
                "source" | "interpretation" | "hypothesis" | "imagination"
            ) && (!matches!(
                g.kind.as_str(),
                "source" | "interpretation"
            ) || assoc
                .group_root_indptr[i]
                < assoc.group_root_indptr[i + 1])),
            "Invalid support group premises, keys or kind"
        );
        Ok((c, t, f, a))
    }
}
