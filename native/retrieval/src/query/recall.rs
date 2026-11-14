//! Scope-aware group recall and fixed candidate set calibration.
use super::*;

impl Snapshot {
    pub(super) fn recall_context(
        &self,
        allowed: &BTreeSet<u64>,
        options: &Options,
        cancel: &CancellationToken,
    ) -> Result<CandidateSet> {
        let mut sets = Vec::new();
        for fragment in &options.context_lexical {
            ensure!(
                fragment.contribution.is_finite() && fragment.contribution >= 0.0,
                "Invalid lexical context contribution"
            );
            if fragment.contribution == 0.0 || fragment.text.trim().is_empty() {
                continue;
            }
            let lane = format!("context:{}", fragment.key);
            let hits = self.lexical(
                &fragment.text,
                allowed,
                options.readout.bm25_k,
                options.candidates_per_group,
                options,
                &lane,
                cancel,
            )?;
            let mut set = self.lane_set(&lane, hits, options);
            for entry in &mut set.entries {
                if let Some(score) = entry.lanes.get_mut(&lane) {
                    score.weight = fragment.contribution;
                }
            }
            sets.push(set);
        }
        self.union_sets(
            &sets.iter().collect::<Vec<_>>(),
            options.readout.max_union_candidates,
            None,
            options.candidates_per_group,
            options,
        )
    }

    pub(super) fn lexical(
        &self,
        text: &str,
        allowed: &BTreeSet<u64>,
        limit: usize,
        candidates_per_group: usize,
        o: &Options,
        lane: &str,
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
                candidates_per_group,
                self.inner.group_ranks.clone(),
                cancel,
            )?;
            o.lane_coverage
                .lock()
                .unwrap()
                .insert(lane.into(), coverage);
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
        candidates_per_group: usize,
        search_effort: usize,
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
            .map(|r| self.inner.candidate_ids[*r as usize])
            .collect::<BTreeSet<_>>();
        if o.grouping != "document" {
            cancel.check()?;
            return Ok(ann
                .search_candidates(vector, limit, Some(&ids), search_effort)?
                .into_iter()
                .map(|h| {
                    self.inner
                        .candidate_rows
                        .get(&h.id)
                        .copied()
                        .map(|row| (row, h.similarity))
                        .ok_or_else(|| {
                            failure("wave.invalid_snapshot", "Unknown ANN candidate identity")
                        })
                })
                .collect::<Result<Vec<_>>>()?);
        }
        let groups = &self.inner.candidates.group_rows;
        let mut remaining = ids;
        let mut seen = BTreeSet::new();
        let mut counts = BTreeMap::<u64, usize>::new();
        let mut hits = Vec::new();
        let mut no_progress = false;
        while !remaining.is_empty() && seen.len() < o.group_scan_limit {
            cancel.check()?;
            let batch_size = (limit.saturating_mul(candidates_per_group))
                .max(32)
                .min(o.group_scan_limit - seen.len());
            let batch =
                ann.search_candidates(vector, batch_size, Some(&remaining), search_effort)?;
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
                let row = *self.inner.candidate_rows.get(&hit.id).ok_or_else(|| {
                    failure("wave.invalid_snapshot", "Unknown ANN candidate identity")
                })?;
                let group = groups[row as usize];
                if !counts.contains_key(&group) && counts.len() >= limit {
                    continue;
                }
                let count = counts.entry(group).or_default();
                if *count < candidates_per_group {
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
                    .get(&groups[self.inner.candidate_rows[id] as usize])
                    .is_none_or(|count| *count < candidates_per_group)
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
    pub(super) fn lane_set(
        &self,
        lane: &str,
        hits: Vec<(u64, f64)>,
        options: &Options,
    ) -> CandidateSet {
        CandidateSet {
            rank_scope: if options.grouping == "document" {
                "grouped_lane".into()
            } else {
                candidate_rank_scope()
            },
            identity: self.inner.input.identity.clone(),
            offered_candidates: hits.len(),
            entries: hits
                .into_iter()
                .enumerate()
                .map(|(rank, (row, score))| PoolEntry {
                    row,
                    lanes: [(
                        lane.into(),
                        LaneScore {
                            score,
                            rank: rank + 1,
                            weight: 1.0,
                        },
                    )]
                    .into_iter()
                    .collect(),
                })
                .collect(),
            lane_coverage: options.lane_coverage.lock().unwrap().clone(),
        }
    }

    pub(super) fn direct_lane(
        &self,
        observation: &QueryObservation,
        allowed: &BTreeSet<u64>,
        limit: usize,
        candidates_per_group: usize,
        options: &Options,
        cancel: &CancellationToken,
    ) -> Result<CandidateSet> {
        self.validate_observation(observation)?;
        let direct = observation
            .core_rows
            .iter()
            .copied()
            .chain(
                observation
                    .output
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
        let mut anchors = direct
            .iter()
            .flat_map(|r| self.inner.concept_candidates.get(r).into_iter().flatten())
            .copied()
            .filter(|r| allowed.contains(r))
            .collect::<BTreeSet<_>>()
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
        if options.grouping == "document" && limit > 0 {
            let eligible = anchors
                .iter()
                .map(|(row, _)| self.inner.candidates.group_rows[*row as usize])
                .collect::<BTreeSet<_>>()
                .len();
            options.lane_coverage.lock().unwrap().insert(
                "direct_anchor".into(),
                crate::grouping::LaneCoverage {
                    examined_rows: anchors.len(),
                    eligible_groups_known: Some(eligible),
                    recall_exhausted: true,
                    truncated: eligible > limit,
                    reason: (eligible > limit).then(|| "lane_group_limit".into()),
                },
            );
        }
        let hits = if options.grouping == "document" {
            crate::grouping::collect(
                anchors,
                &self.inner.candidates.group_rows,
                limit,
                candidates_per_group,
            )
        } else {
            anchors.into_iter().take(limit).collect()
        };
        cancel.check()?;
        Ok(self.lane_set("direct_anchor", hits, options))
    }

    pub(super) fn union_sets(
        &self,
        sets: &[&CandidateSet],
        candidate_limit: usize,
        group_limit: Option<usize>,
        candidates_per_group: usize,
        options: &Options,
    ) -> Result<CandidateSet> {
        let mut rows = BTreeMap::<u64, BTreeMap<String, LaneScore>>::new();
        let mut coverage = BTreeMap::new();
        for set in sets {
            self.validate_candidates(set)?;
            coverage.extend(set.lane_coverage.clone());
            for entry in &set.entries {
                let lanes = rows.entry(entry.row).or_default();
                for (name, score) in &entry.lanes {
                    let existing: &mut LaneScore =
                        lanes.entry(name.clone()).or_insert_with(|| score.clone());
                    if score.rank < existing.rank
                        || score.rank == existing.rank && score.score > existing.score
                    {
                        *existing = score.clone();
                    }
                }
            }
        }
        let mut entries = rows
            .into_iter()
            .map(|(row, lanes)| PoolEntry { row, lanes })
            .collect::<Vec<_>>();
        let score = |entry: &PoolEntry| {
            entry
                .lanes
                .values()
                .map(|lane| lane.weight / (options.rrf_k + lane.rank as f64))
                .sum::<f64>()
        };
        entries.sort_by(|a, b| score(b).total_cmp(&score(a)).then(a.row.cmp(&b.row)));
        let offered = entries.len();
        if options.grouping == "document" {
            let mut group_lanes = BTreeMap::<u64, BTreeMap<String, (usize, f64)>>::new();
            for entry in &entries {
                let lanes = group_lanes
                    .entry(self.inner.candidates.group_rows[entry.row as usize])
                    .or_default();
                for (name, lane) in &entry.lanes {
                    let rank = lanes
                        .entry(name.clone())
                        .or_insert((lane.rank, lane.weight));
                    if lane.rank < rank.0 {
                        *rank = (lane.rank, lane.weight);
                    }
                }
            }
            let mut groups = group_lanes
                .into_iter()
                .map(|(group, lanes)| {
                    (
                        group,
                        lanes
                            .values()
                            .map(|(rank, weight)| weight / (options.rrf_k + *rank as f64))
                            .sum::<f64>(),
                    )
                })
                .collect::<Vec<_>>();
            groups.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let groups = groups
                .into_iter()
                .take(group_limit.unwrap_or(usize::MAX))
                .enumerate()
                .map(|(rank, (group, _))| (group, rank))
                .collect::<BTreeMap<_, _>>();
            entries.retain(|entry| {
                groups.contains_key(&self.inner.candidates.group_rows[entry.row as usize])
            });
            entries.sort_by(|a, b| {
                groups[&self.inner.candidates.group_rows[a.row as usize]]
                    .cmp(&groups[&self.inner.candidates.group_rows[b.row as usize]])
                    .then_with(|| score(b).total_cmp(&score(a)))
                    .then(a.row.cmp(&b.row))
            });
            let ordered = entries.iter().map(|entry| entry.row).collect::<Vec<_>>();
            let selected = crate::grouping::fair_rows(
                &ordered,
                &self.inner.candidates.group_rows,
                candidate_limit,
                candidates_per_group,
            );
            entries.retain(|entry| selected.contains(&entry.row));
        } else {
            entries.truncate(candidate_limit);
        }
        Ok(CandidateSet {
            rank_scope: if options.grouping == "document" {
                "grouped_lane".into()
            } else {
                candidate_rank_scope()
            },
            identity: self.inner.input.identity.clone(),
            entries,
            offered_candidates: offered,
            lane_coverage: coverage,
        })
    }

    pub(super) fn calibrate(
        &self,
        observation: &QueryObservation,
        input: &CandidateSet,
        options: &Options,
    ) -> Result<CalibratedPool> {
        self.validate_observation(observation)?;
        self.validate_candidates(input)?;
        let prepared = self.prepare_pool(
            observation,
            input,
            &options.readout,
            options.grouping == "document",
        )?;
        let mut candidates = input.clone();
        candidates.entries.retain(|entry| {
            prepared
                .selection
                .iter()
                .any(|part| part.id == entry.row as i64 + 1)
        });
        candidates.entries.sort_by_key(|entry| {
            prepared
                .selection
                .iter()
                .position(|part| part.id == entry.row as i64 + 1)
                .unwrap()
        });
        Ok(CalibratedPool {
            candidates,
            prepared,
            observation: observation.output.clone(),
            population: input.entries.len(),
        })
    }
}
