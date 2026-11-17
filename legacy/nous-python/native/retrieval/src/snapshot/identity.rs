//! Stable internal IDs for immutable snapshot components.
use super::CandidateMeta;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StableIds {
    pub candidates: BTreeMap<String, i64>,
    pub groups: BTreeMap<String, u64>,
    next_candidate: i64,
    next_group: u64,
}
impl StableIds {
    pub fn build(meta: &CandidateMeta, previous: Option<&Self>) -> Result<Self> {
        let mut ids = previous.cloned().unwrap_or(Self {
            candidates: BTreeMap::new(),
            groups: BTreeMap::new(),
            next_candidate: 1,
            next_group: 1,
        });
        let keys: BTreeSet<_> = meta.keys.iter().collect();
        let groups: BTreeSet<_> = meta.group_keys.iter().collect();
        ids.candidates.retain(|key, _| keys.contains(key));
        ids.groups.retain(|key, _| groups.contains(key));
        for key in &meta.keys {
            if !ids.candidates.contains_key(key) {
                ids.candidates.insert(key.clone(), ids.next_candidate);
                ids.next_candidate = ids
                    .next_candidate
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("candidate identity exhausted"))?;
            }
        }
        for key in &meta.group_keys {
            if !ids.groups.contains_key(key) {
                ids.groups.insert(key.clone(), ids.next_group);
                ids.next_group = ids
                    .next_group
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("group identity exhausted"))?;
            }
        }
        ids.validate(meta)?;
        Ok(ids)
    }
    pub fn validate(&self, meta: &CandidateMeta) -> Result<()> {
        ensure!(
            self.candidates.len() == meta.keys.len()
                && meta
                    .keys
                    .iter()
                    .all(|key| self.candidates.contains_key(key)),
            "candidate identity keys disagree"
        );
        ensure!(
            self.groups.len() == meta.group_keys.len()
                && meta
                    .group_keys
                    .iter()
                    .all(|key| self.groups.contains_key(key)),
            "group identity keys disagree"
        );
        ensure!(
            self.candidates
                .values()
                .all(|id| *id > 0 && *id < self.next_candidate)
                && self.candidates.values().collect::<BTreeSet<_>>().len() == self.candidates.len(),
            "invalid candidate identities"
        );
        ensure!(
            self.groups
                .values()
                .all(|id| *id > 0 && *id < self.next_group)
                && self.groups.values().collect::<BTreeSet<_>>().len() == self.groups.len(),
            "invalid group identities"
        );
        Ok(())
    }
    pub fn group_for(&self, meta: &CandidateMeta, row: usize) -> u64 {
        self.groups[&meta.group_keys[meta.group_rows[row] as usize]]
    }
}
