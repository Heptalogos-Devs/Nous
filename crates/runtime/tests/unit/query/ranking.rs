// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::MemoryId;

#[test]
fn denominator_is_plan_family_based() {
    let candidate = CandidateRankInput {
        reference: CognitiveRef::Memory(MemoryId::new()),
        family_ranks: HashMap::from([(EvidenceFamily::Lexical, 1)]),
        family_view_ranks: HashMap::new(),
        variants: Vec::new(),
    };
    let reference = candidate.reference.clone();
    let mut extra = candidate.clone();
    extra.reference = CognitiveRef::Memory(MemoryId::new());
    extra.family_ranks.insert(EvidenceFamily::Dense, 1);
    let first = rank_candidates(std::slice::from_ref(&candidate), &[EvidenceFamily::Lexical])[0]
        .baseline_score;
    let second = rank_candidates(&[candidate, extra], &[EvidenceFamily::Lexical])
        .into_iter()
        .find(|value| value.reference == reference)
        .unwrap()
        .baseline_score;
    assert_eq!(first, second);
}

#[test]
fn multi_view_family_uses_one_best_rank_and_tie_order_is_stable() {
    let left = CandidateRankInput {
        reference: CognitiveRef::Memory(MemoryId::new()),
        family_ranks: HashMap::new(),
        family_view_ranks: HashMap::from([(EvidenceFamily::Dense, vec![7, 2, 5])]),
        variants: vec!["dense:space-a".into()],
    };
    let right = CandidateRankInput {
        reference: CognitiveRef::Memory(MemoryId::new()),
        family_ranks: HashMap::from([(EvidenceFamily::Dense, 2)]),
        family_view_ranks: HashMap::new(),
        variants: Vec::new(),
    };
    let reverse = rank_candidates(&[right.clone(), left.clone()], &[EvidenceFamily::Dense]);
    let forward = rank_candidates(&[left, right], &[EvidenceFamily::Dense]);
    let reverse_refs = reverse
        .iter()
        .map(|candidate| candidate.reference.to_string())
        .collect::<Vec<_>>();
    let forward_refs = forward
        .iter()
        .map(|candidate| candidate.reference.to_string())
        .collect::<Vec<_>>();
    assert_eq!(reverse_refs, forward_refs);
}
