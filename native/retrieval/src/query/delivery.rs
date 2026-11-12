//! Group and materialize ranked candidates with evidence and coverage.
use super::*;

pub(super) struct RankedCandidates {
    pub hits: Vec<Value>,
    pub channel_scores: BTreeMap<u64, BTreeMap<String, f64>>,
    pub channel_ranks: BTreeMap<u64, BTreeMap<String, usize>>,
    pub offered_candidates: usize,
    pub diagnostics: Value,
    pub thinking: Value,
    pub score_kind: &'static str,
    pub wormhole_rrf_active: bool,
    pub used_lanes: Vec<String>,
    pub skipped: Vec<Value>,
    pub started: std::time::Instant,
    pub readout_max_union_candidates: usize,
    pub local_vector: Option<Vec<f32>>,
}

impl Snapshot {
    pub(super) fn deliver(
        &self,
        mode: &str,
        text: &str,
        allowed: &BTreeSet<u64>,
        o: &Options,
        provided_pool: Option<&CandidatePool>,
        cancel: &CancellationToken,
        ranked: RankedCandidates,
    ) -> Result<NativeQueryResult> {
        let RankedCandidates {
            mut hits,
            channel_scores,
            channel_ranks,
            mut offered_candidates,
            mut diagnostics,
            thinking,
            score_kind,
            wormhole_rrf_active,
            mut used_lanes,
            skipped,
            started,
            readout_max_union_candidates,
            local_vector,
        } = ranked;
        cancel.check()?;
        if o.grouping == "document"
            && provided_pool.is_none()
            && hits.len() > readout_max_union_candidates
        {
            offered_candidates = offered_candidates.max(hits.len());
            let ordered = hits
                .iter()
                .map(|hit| hit["row"].as_u64().unwrap())
                .collect::<Vec<_>>();
            let selected = crate::grouping::fair_rows(
                &ordered,
                &self.inner.candidates.group_rows,
                readout_max_union_candidates,
                o.passages_per_group,
            );
            hits.retain(|hit| selected.contains(&hit["row"].as_u64().unwrap()));
        }
        let ranked_candidates = hits.len();
        if offered_candidates == 0 {
            offered_candidates = ranked_candidates;
        }
        let dedup_vectors = if o.grouping == "candidate" && o.dedup_threshold.is_some() {
            self.inner.candidate_units.as_ref().map(|vectors| {
                hits.iter()
                    .map(|hit| vectors[hit["row"].as_u64().unwrap() as usize].as_ref())
                    .collect::<Vec<_>>()
            })
        } else {
            None
        };
        let mut representatives = crate::ordered_dedup::OrderedDedup::default();
        let mut kept = Vec::<Value>::new();
        let threshold = o.dedup_threshold;
        if let Some(v) = threshold {
            ensure!(
                v.is_finite() && (-1.0..=1.0).contains(&v),
                "Dedup threshold invalid"
            );
        }
        for (hit_index, mut hit) in hits.into_iter().enumerate() {
            cancel.check()?;
            let row = hit["row"].as_u64().unwrap() as usize;
            let mut folded = false;
            if let (Some(threshold), Some(vectors)) = (threshold, dedup_vectors.as_ref()) {
                let vector = &vectors[hit_index];
                if let Some((index, cosine)) = representatives.first_match(vector, threshold) {
                    kept[index]["folded_rows"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"row":row,"similarity":cosine}));
                    folded = true;
                } else {
                    representatives.push(vector);
                }
            }
            if folded {
                continue;
            }
            let concepts = slice(
                &self.inner.input.candidates.membership_indptr,
                &self.inner.input.candidates.membership_indices,
                row,
            );
            hit["concept_rows"] = json!(concepts);
            hit["evidence_roots"] = json!(slice(
                &self.inner.input.candidates.root_indptr,
                &self.inner.input.candidates.root_indices,
                row
            ));
            if o.include_trace {
                if let Some(assets) = &self.inner.assets {
                    let graph = assets.graph();
                    let roots = slice(
                        &self.inner.input.candidates.root_indptr,
                        &self.inner.input.candidates.root_indices,
                        row,
                    )
                    .iter()
                    .map(|r| *r as i64 + 1)
                    .collect::<Vec<_>>();
                    let mut support = Vec::new();
                    for concept in concepts {
                        let source = *concept as i64 + 1;
                        if let Some(index) = graph.node_index.get(&source) {
                            for cursor in graph.row_offsets[*index]..graph.row_offsets[*index + 1] {
                                let target = graph.node_ids[graph.targets[cursor]];
                                if concepts.contains(&((target - 1) as u64)) {
                                    support.push(json!({"source":source-1,"target":target-1,"independent_support_fraction":graph.independent_support_fraction(source,target,&roots),"support_weight":graph.independent_fraction_excluding(source,target,&roots)}));
                                }
                            }
                        }
                    }
                    hit["independent_support"] = json!(support);
                }
            }
            hit["folded_rows"] = json!([]);
            hit["counterevidence_links"] = json!([]);
            hit["channel_ranks"] = json!(channel_ranks.get(&(row as u64)));
            hit["channel_scores"] = json!(channel_scores.get(&(row as u64)));
            hit["score_kind"] = json!(score_kind);
            hit["lexical_evidence"] = self.inner.lexical.evidence(
                text,
                &self.inner.input.candidates.texts[row],
                &self.inner.candidates.titles[row],
            );
            hit["rank"] = json!(kept.len() + 1);
            if o.include_counterevidence {
                let targets = concepts
                    .iter()
                    .flat_map(|c| self.inner.negative_neighbors.get(c).into_iter().flatten())
                    .copied()
                    .collect::<BTreeSet<_>>();
                let rows = targets
                    .iter()
                    .flat_map(|t| self.inner.concept_candidates.get(t).into_iter().flatten())
                    .filter(|r| **r != row as u64 && allowed.contains(r))
                    .copied()
                    .collect::<BTreeSet<_>>();
                let links = rows
                    .into_iter()
                    .map(|related| {
                        let related_concepts = slice(
                            &self.inner.input.candidates.membership_indptr,
                            &self.inner.input.candidates.membership_indices,
                            related as usize,
                        );
                        let associations = concepts
                            .iter()
                            .flat_map(|source| {
                                related_concepts.iter().flat_map(move |target| {
                                    self.inner
                                        .negative_relations
                                        .get(&(*source, *target))
                                        .into_iter()
                                        .flatten()
                                })
                            })
                            .copied()
                            .collect::<BTreeSet<_>>();
                        json!({"row":related,"associations":associations})
                    })
                    .collect::<Vec<_>>();
                hit["counterevidence_links"] = json!(links);
            }
            kept.push(hit);
        }
        let before = kept.len();
        let all_groups = kept
            .iter()
            .map(|hit| self.inner.candidates.group_rows[hit["row"].as_u64().unwrap() as usize])
            .collect::<BTreeSet<_>>();
        let mut groups = Vec::<Value>::new();
        if o.grouping == "document" {
            // Keep the final readout order: the first candidate establishes each document rank.
            let mut group_indices = BTreeMap::<u64, usize>::new();
            let mut selected = Vec::new();
            for (index, mut hit) in kept.into_iter().enumerate() {
                let row = hit["row"].as_u64().unwrap();
                let group = self.inner.candidates.group_rows[row as usize];
                if !group_indices.contains_key(&group) {
                    if groups.len() >= o.top_k {
                        continue;
                    }
                    group_indices.insert(group, groups.len());
                    groups.push(json!({"group_row":group,"group_key":self.inner.candidates.group_keys[group as usize],"group_rank":groups.len()+1,"rank_basis_row":row,"passage_rows":[]}));
                }
                let group_index = group_indices[&group];
                let passages = groups[group_index]["passage_rows"].as_array_mut().unwrap();
                if passages.len() >= o.passages_per_group {
                    continue;
                }
                passages.push(json!(row));
                hit["candidate_rank"] = json!(index + 1);
                hit["group_rank"] = json!(group_index + 1);
                hit["group_key"] = json!(self.inner.candidates.group_keys[group as usize]);
                selected.push(hit);
            }
            kept = selected;
            // A group's rank and its readable location are separate facts. Inspect only
            // allowed local rows of selected groups; never rerun global recall or VCP.
            let selected_groups = groups
                .iter()
                .filter_map(|group| group["group_row"].as_u64())
                .collect::<BTreeSet<_>>();
            let mut local_rows = BTreeMap::<u64, Vec<u64>>::new();
            for row in allowed {
                let group = self.inner.candidates.group_rows[*row as usize];
                if selected_groups.contains(&group) && self.inner.candidates.local[*row as usize] {
                    let rows = local_rows.entry(group).or_default();
                    if rows.len() < o.group_scan_limit {
                        rows.push(*row);
                    }
                }
            }
            for group in &mut groups {
                cancel.check()?;
                let group_row = group["group_row"].as_u64().unwrap();
                let basis_row = group["rank_basis_row"].as_u64().unwrap();
                let basis = kept
                    .iter()
                    .find(|hit| hit["row"].as_u64() == Some(basis_row))
                    .unwrap();
                let evidence = basis["lexical_evidence"].clone();
                let fields = evidence["matched_fields"].as_array().unwrap();
                let reason = if !evidence["body_range"].is_null() {
                    "body_match"
                } else if fields.iter().any(|field| field.as_str() == Some("title")) {
                    "title_preview"
                } else if self.inner.candidates.local[basis_row as usize] {
                    "local_score"
                } else {
                    "preview"
                };
                let mut passages = vec![
                    json!({"row":basis_row,"reason":reason,"lexical_evidence":evidence,"evidence_row":basis_row}),
                ];
                if reason == "preview" {
                    let ranked_local = group["passage_rows"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_u64)
                        .find(|row| self.inner.candidates.local[*row as usize]);
                    if let Some(row) = ranked_local {
                        let hit = kept
                            .iter()
                            .find(|hit| hit["row"].as_u64() == Some(row))
                            .unwrap();
                        let lexical = hit["lexical_evidence"].clone();
                        let selection_reason = if lexical["body_range"].is_null() {
                            "local_score"
                        } else {
                            "body_match"
                        };
                        passages[0] = json!({"row":row,"reason":selection_reason,"lexical_evidence":lexical,"evidence_row":row});
                    }
                    let mut best: Option<(u64, f64, Value, &str)> = None;
                    for candidate in local_rows
                        .get(&group_row)
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|_| ranked_local.is_none())
                    {
                        cancel.check()?;
                        let row = candidate as usize;
                        let lexical = self.inner.lexical.evidence(
                            text,
                            &self.inner.input.candidates.texts[row],
                            &self.inner.candidates.titles[row],
                        );
                        if !lexical["body_range"].is_null() {
                            best = Some((candidate, 0.0, lexical, "body_match"));
                            break;
                        }
                        if matches!(mode, "dense" | "tagmemo") {
                            if let (Some(query), Some(vectors)) =
                                (local_vector.as_ref(), self.inner.candidate_units.as_ref())
                            {
                                let score: f64 = vectors[row]
                                    .iter()
                                    .zip(query)
                                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                                    .sum();
                                if best.as_ref().is_none_or(|(_, old, _, _)| score > *old) {
                                    best = Some((candidate, score, lexical, "local_score"));
                                }
                            }
                        }
                    }
                    if let Some((row, _, lexical, selection_reason)) = best {
                        let ranked = kept.iter().find(|hit| hit["row"].as_u64() == Some(row));
                        passages[0] = json!({"row":row,"reason":selection_reason,"lexical_evidence":lexical,"evidence_row":ranked.map(|_|row)});
                    }
                }
                for row in group["passage_rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_u64)
                {
                    if passages.len() >= o.passages_per_group {
                        break;
                    }
                    if passages
                        .iter()
                        .any(|part| part["row"].as_u64() == Some(row))
                    {
                        continue;
                    }
                    if reason == "preview" && row == basis_row && passages[0]["row"] != basis_row {
                        continue;
                    }
                    let hit = kept
                        .iter()
                        .find(|hit| hit["row"].as_u64() == Some(row))
                        .unwrap();
                    let lexical = hit["lexical_evidence"].clone();
                    let reason = if !lexical["body_range"].is_null() {
                        "body_match"
                    } else if self.inner.candidates.local[row as usize] {
                        "local_score"
                    } else {
                        "preview"
                    };
                    passages.push(json!({"row":row,"reason":reason,"lexical_evidence":lexical,"evidence_row":row}));
                }
                group["passages"] = json!(passages);
                group.as_object_mut().unwrap().remove("passage_rows");
            }
        } else {
            kept.truncate(o.top_k);
        }
        let lane_coverage = o.lane_coverage.lock().unwrap().clone();
        let recall_exhausted =
            !lane_coverage.is_empty() && lane_coverage.values().all(|lane| lane.recall_exhausted);
        let recall_truncated = lane_coverage.values().any(|lane| lane.truncated);
        let pool_truncated = offered_candidates > readout_max_union_candidates;
        let top_k_truncated = if o.grouping == "document" {
            all_groups.len() > o.top_k
        } else {
            before > o.top_k
        };
        let eligible_groups_known = if lane_coverage.len() == 1 {
            lane_coverage.values().next().unwrap().eligible_groups_known
        } else if recall_exhausted && !recall_truncated && !pool_truncated {
            Some(all_groups.len())
        } else {
            None
        };
        let coverage_reason = if pool_truncated {
            Some("max_union_candidates".to_string())
        } else if recall_truncated {
            lane_coverage.values().find_map(|lane| lane.reason.clone())
        } else if top_k_truncated {
            Some("top_k".to_string())
        } else {
            None
        };
        let rows = kept.iter().map(|v| v["row"].as_u64().unwrap()).collect();
        let quality = kept
            .iter()
            .map(|v| v["quality_score"].as_f64().unwrap_or(f64::NAN))
            .collect();
        let ranking = kept
            .iter()
            .map(|v| v["ranking_score"].as_f64().unwrap())
            .collect();
        let mut reported_lanes = BTreeSet::new();
        used_lanes.retain(|lane| reported_lanes.insert(lane.clone()));
        diagnostics["total_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        diagnostics["max_union_candidates"] = json!(readout_max_union_candidates);
        diagnostics["readout_max_union_candidates"] = json!(readout_max_union_candidates);
        diagnostics["input_scale"] = json!({"allowed_candidates":allowed.len(),"concepts":self.inner.concepts.keys.len(),"max_union_candidates":readout_max_union_candidates});
        Ok(NativeQueryResult{rows,quality,ranking,result_json:json!({"mode":mode,"score_kind":score_kind,"wormhole_rrf_active":wormhole_rrf_active,"used_lanes":used_lanes,"skipped_lanes":skipped,"hits":kept,"groups":groups,"grouping":o.grouping,"rank_scope":provided_pool.map(|pool|pool.rank_scope.as_str()).unwrap_or(if o.grouping == "document" {"grouped_lane"} else {"candidate_lane"}),"thinking":thinking,"algorithm_versions":{"vcp_upstream":"d5c019ebbac1f317c2938d655d2ce916e41b9164","pipeline":"memo.conservative-topology-v2.2","river":"rivermemo.conservative-topology-v2.2"},"coverage":{"requested_groups":if o.grouping=="document"{Some(o.top_k)}else{None},"returned_groups":groups.len(),"eligible_groups_known":eligible_groups_known,"allowed_groups":allowed.iter().map(|row|self.inner.candidates.group_rows[*row as usize]).collect::<BTreeSet<_>>().len(),"recall_exhausted":recall_exhausted,"truncated":recall_truncated||pool_truncated||top_k_truncated,"reason":coverage_reason,"lanes":lane_coverage,"lane_limit_unit":if o.grouping == "document" {"groups"}else{"candidates"},"group_scan_limit":o.group_scan_limit,"passages_per_group":o.passages_per_group,"candidates":before,"total_candidates":self.inner.candidates.keys.len(),"max_union_candidates":readout_max_union_candidates,"top_k":o.top_k,"offered_candidates":offered_candidates,"ranked_candidates":ranked_candidates,"unique_candidates":before,"candidate_limit_reached":offered_candidates>readout_max_union_candidates,"top_k_truncated":top_k_truncated},"diagnostics":if o.include_trace{diagnostics}else{Value::Null}}).to_string()})
    }
}
