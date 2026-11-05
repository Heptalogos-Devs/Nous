//! Scope-aware group recall and fixed candidate pool calibration.
use super::*;

impl Snapshot {
    pub(super) fn lexical(
        &self,
        text: &str,
        allowed: &BTreeSet<u64>,
        limit: usize,
        o: &Options,
        cancel: &CancellationToken,
    ) -> Result<Vec<(u64, f64)>> {
        if limit == 0 || text.trim().is_empty() {
            return Ok(vec![]);
        }
        cancel.check()?;
        let refs = allowed
            .iter()
            .map(|r| self.inner.candidates.keys[*r as usize].clone())
            .collect();
        let lookup = self
            .inner
            .candidates
            .keys
            .iter()
            .enumerate()
            .map(|(r, k)| (k, r as u64))
            .collect::<BTreeMap<_, _>>();
        let page = if o.grouping == "document" {
            let (page, coverage) = self.inner.lexical.search_grouped(
                text,
                &refs,
                limit,
                o.passages_per_group,
                cancel,
            )?;
            o.lane_coverage
                .lock()
                .unwrap()
                .insert("bm25".into(), coverage);
            page
        } else {
            self.inner
                .lexical
                .search_allowed(text, "snapshot", "snapshot", limit, &refs)?
        };
        Ok(page
            .hits
            .into_iter()
            .map(|h| (lookup[&h.id], f64::from(h.score)))
            .collect())
    }
    pub(super) fn dense(
        &self,
        vector: &[f32],
        allowed: &BTreeSet<u64>,
        limit: usize,
        o: &Options,
        lane: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<(u64, f64)>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let ann = self
            .inner
            .ann
            .as_ref()
            .ok_or_else(|| failure("wave.capability_missing", "Dense vectors unavailable"))?;
        let ids = allowed
            .iter()
            .map(|r| *r as i64 + 1)
            .collect::<BTreeSet<_>>();
        if o.grouping != "document" {
            cancel.check()?;
            return Ok(ann
                .search_candidates(vector, limit, Some(&ids))?
                .into_iter()
                .map(|h| ((h.id - 1) as u64, h.similarity))
                .collect());
        }
        let groups = &self.inner.candidates.group_rows;
        let mut remaining = ids;
        let mut seen = BTreeSet::new();
        let mut counts = BTreeMap::<u64, usize>::new();
        let mut hits = Vec::new();
        let mut no_progress = false;
        while !remaining.is_empty() && seen.len() < o.group_scan_limit {
            cancel.check()?;
            let batch_size = (limit.saturating_mul(o.passages_per_group))
                .max(32)
                .min(o.group_scan_limit - seen.len());
            let batch = ann.search_candidates(vector, batch_size, Some(&remaining))?;
            if batch.is_empty() {
                no_progress = true;
                break;
            }
            let mut added = 0;
            for hit in batch {
                if !seen.insert(hit.id) {
                    continue;
                }
                added += 1;
                remaining.remove(&hit.id);
                let row = (hit.id - 1) as u64;
                let group = groups[row as usize];
                if !counts.contains_key(&group) && counts.len() >= limit {
                    continue;
                }
                let count = counts.entry(group).or_default();
                if *count < o.passages_per_group {
                    *count += 1;
                    hits.push((row, hit.similarity));
                }
            }
            if added == 0 {
                no_progress = true;
                break;
            }
            remaining.retain(|id| {
                counts
                    .get(&groups[(*id - 1) as usize])
                    .is_none_or(|count| *count < o.passages_per_group)
            });
            if counts.len() >= limit {
                break;
            }
        }
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let exhausted = seen.len() == allowed.len();
        let scan_limited = seen.len() >= o.group_scan_limit && !exhausted;
        let target_reached = counts.len() >= limit && !exhausted;
        o.lane_coverage.lock().unwrap().insert(
            lane.into(),
            crate::grouping::LaneCoverage {
                examined_rows: seen.len(),
                eligible_groups_known: (exhausted || remaining.is_empty()).then_some(counts.len()),
                recall_exhausted: exhausted,
                truncated: scan_limited || target_reached || no_progress,
                reason: if scan_limited {
                    Some("group_scan_limit".into())
                } else if target_reached {
                    Some("lane_group_limit".into())
                } else if no_progress {
                    Some("ann_no_progress".into())
                } else {
                    None
                },
            },
        );
        Ok(hits)
    }
    pub fn recall(
        &self,
        observation: &QueryObservation,
        allowed: &BTreeSet<u64>,
        o: &Options,
        cancel: &CancellationToken,
    ) -> Result<CandidatePool> {
        self.validate_observation(observation)?;
        o.validate_grouping()?;
        if allowed
            .iter()
            .any(|r| *r >= self.inner.candidates.keys.len() as u64)
        {
            return Err(failure(
                "wave.invalid_input",
                "Candidate scope contains a row outside this snapshot",
            ));
        }
        o.readout
            .validate()
            .map_err(|e| failure("wave.invalid_input", e))?;
        let mut entries = BTreeMap::<u64, BTreeMap<String, LaneScore>>::new();
        let output = &observation.output;
        for (name, vector) in [
            ("query_knn", &output.original_query_vector),
            ("denoised_field_knn", &output.enhanced_vector),
            ("local_field_knn", &output.local_vector),
            ("transfer_field_knn", &output.transfer_vector),
        ] {
            if o.lane_limit(name) > 0 && vector.iter().any(|v| *v != 0.0) {
                for (rank, (row, score)) in self
                    .dense(vector, allowed, o.lane_limit(name), o, name, cancel)?
                    .into_iter()
                    .enumerate()
                {
                    entries.entry(row).or_default().insert(
                        name.into(),
                        LaneScore {
                            score,
                            rank: rank + 1,
                        },
                    );
                }
            }
        }
        for (rank, (row, score)) in self
            .lexical(&observation.text, allowed, o.lane_limit("bm25"), o, cancel)?
            .into_iter()
            .enumerate()
        {
            entries.entry(row).or_default().insert(
                "bm25".into(),
                LaneScore {
                    score,
                    rank: rank + 1,
                },
            );
        }
        let direct = observation
            .core_rows
            .iter()
            .copied()
            .chain(
                output
                    .observation
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.hop == 0
                            && matches!(n.source_type.as_str(), "core" | "lexical_seed" | "seed")
                    })
                    .map(|n| (n.id - 1) as u64),
            )
            .collect::<BTreeSet<_>>();
        let anchor_rows = direct
            .iter()
            .flat_map(|r| self.inner.concept_candidates.get(r).into_iter().flatten())
            .copied()
            .filter(|r| allowed.contains(r))
            .collect::<BTreeSet<_>>();
        let mut anchors = anchor_rows
            .into_iter()
            .map(|row| {
                let concepts = slice(
                    &self.inner.input.candidates.membership_indptr,
                    &self.inner.input.candidates.membership_indices,
                    row as usize,
                );
                (
                    row,
                    concepts.iter().filter(|c| direct.contains(c)).count() as f64,
                )
            })
            .collect::<Vec<_>>();
        anchors.sort_by(|a, b| {
            b.1.total_cmp(&a.1).then(
                self.inner.candidates.keys[a.0 as usize]
                    .cmp(&self.inner.candidates.keys[b.0 as usize]),
            )
        });
        if o.grouping == "document" && o.lane_limit("direct_anchor") > 0 {
            let eligible = anchors
                .iter()
                .map(|(row, _)| self.inner.candidates.group_rows[*row as usize])
                .collect::<BTreeSet<_>>()
                .len();
            o.lane_coverage.lock().unwrap().insert(
                "direct_anchor".into(),
                crate::grouping::LaneCoverage {
                    examined_rows: anchors.len(),
                    eligible_groups_known: Some(eligible),
                    recall_exhausted: true,
                    truncated: eligible > o.lane_limit("direct_anchor"),
                    reason: (eligible > o.lane_limit("direct_anchor"))
                        .then(|| "lane_group_limit".into()),
                },
            );
        }
        let anchors = if o.grouping == "document" {
            crate::grouping::collect(
                anchors,
                &self.inner.candidates.group_rows,
                o.lane_limit("direct_anchor"),
                o.passages_per_group,
            )
        } else {
            anchors
                .into_iter()
                .take(o.lane_limit("direct_anchor"))
                .collect()
        };
        for (rank, (row, score)) in anchors.into_iter().enumerate() {
            cancel.check()?;
            entries.entry(row).or_default().insert(
                "direct_anchor".into(),
                LaneScore {
                    score,
                    rank: rank + 1,
                },
            );
        }
        let mut entries = entries
            .into_iter()
            .map(|(row, lanes)| PoolEntry { row, lanes })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            let score = |e: &PoolEntry| {
                e.lanes
                    .values()
                    .map(|l| 1.0 / (o.rrf_k + l.rank as f64))
                    .sum::<f64>()
            };
            score(b).total_cmp(&score(a)).then(a.row.cmp(&b.row))
        });

        if o.grouping == "document" {
            let mut group_lanes = BTreeMap::<u64, BTreeMap<String, usize>>::new();
            for entry in &entries {
                let lanes = group_lanes
                    .entry(self.inner.candidates.group_rows[entry.row as usize])
                    .or_default();
                for (name, lane) in &entry.lanes {
                    let rank = lanes.entry(name.clone()).or_insert(lane.rank);
                    *rank = (*rank).min(lane.rank);
                }
            }
            let group_scores = group_lanes
                .into_iter()
                .map(|(group, lanes)| {
                    (
                        group,
                        lanes
                            .values()
                            .map(|rank| 1.0 / (o.rrf_k + *rank as f64))
                            .sum::<f64>(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            entries.sort_by(|a, b| {
                group_scores[&self.inner.candidates.group_rows[b.row as usize]]
                    .total_cmp(&group_scores[&self.inner.candidates.group_rows[a.row as usize]])
            });
        }
        let mut pool = CandidatePool {
            rank_scope: if o.grouping == "document" {
                "grouped_lane".into()
            } else {
                candidate_rank_scope()
            },
            identity: self.inner.input.identity.clone(),
            offered_candidates: entries.len(),
            entries,
            calibration: None,
            lane_coverage: o.lane_coverage.lock().unwrap().clone(),
        };
        if o.grouping == "document" {
            let ordered = pool
                .entries
                .iter()
                .map(|entry| entry.row)
                .collect::<Vec<_>>();
            let selected = crate::grouping::fair_rows(
                &ordered,
                &self.inner.candidates.group_rows,
                o.readout.max_union_candidates,
                o.passages_per_group,
            );
            pool.entries.retain(|entry| selected.contains(&entry.row));
        }
        let prepared =
            self.prepare_pool(observation, &pool, &o.readout, o.grouping == "document")?;
        pool.entries
            .retain(|e| prepared.selection.iter().any(|c| c.id == e.row as i64 + 1));
        pool.entries.sort_by_key(|e| {
            prepared
                .selection
                .iter()
                .position(|c| c.id == e.row as i64 + 1)
                .unwrap()
        });
        pool.calibration = Some(prepared);
        Ok(pool)
    }
}
