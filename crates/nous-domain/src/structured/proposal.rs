//! Preallocated model targets and independently adopted proposal groups.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see UPSTREAM.md.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredTargetSlot {
    pub key: String,
    pub kind: CognitiveKind,
    pub id: CognitiveId,
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredProposalChange {
    pub slot_key: String,
    pub body: CognitiveContent,
    pub evidence: Vec<EvidenceLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredProposalGroup {
    pub key: String,
    pub depends_on: Vec<String>,
    pub changes: Vec<StructuredProposalChange>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredProposal {
    pub groups: Vec<StructuredProposalGroup>,
}

impl StructuredTargetSlot {
    pub fn output_reference(&self) -> Result<CognitiveRef> {
        self.validate()?;
        let revision = self
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("target revision overflow"))?;
        Ok(CognitiveRef::structured(self.kind, &self.id, revision))
    }
    pub fn validate(&self) -> Result<()> {
        short(&self.key, "target slot key")?;
        validate_uuid(self.id.as_str())?;
        if self.kind == CognitiveKind::Memory || self.expected_revision < 0 {
            return Err(Error::invalid(
                "structured slots need a structured kind and nonnegative expected revision",
            ));
        }
        Ok(())
    }
}
impl StructuredProposalChange {
    pub fn validate(&self) -> Result<()> {
        short(&self.slot_key, "proposal slot key")?;
        self.body.validate(&self.evidence)
    }
    pub fn evidence_links(&self) -> Vec<EvidenceLink> {
        dedup_links(
            self.evidence
                .iter()
                .cloned()
                .chain(self.body.evidence_links())
                .collect(),
        )
    }
    pub fn sources(&self) -> Vec<EvidenceSource> {
        self.evidence_links()
            .into_iter()
            .map(|link| link.source)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
impl StructuredProposalGroup {
    pub fn validate(&self) -> Result<()> {
        short(&self.key, "proposal group key")?;
        if self.changes.is_empty() {
            return Err(Error::invalid("proposal group must contain changes"));
        }
        let mut seen = BTreeSet::new();
        for change in &self.changes {
            change.validate()?;
            if !seen.insert(&change.slot_key) {
                return Err(Error::invalid("duplicate target slot within one group"));
            }
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &self.depends_on {
            short(dependency, "group dependency")?;
            if dependency == &self.key || !dependencies.insert(dependency) {
                return Err(Error::invalid("invalid or duplicate group dependency"));
            }
        }
        Ok(())
    }
}
impl StructuredProposal {
    /// Only proposal topology is global. A caller can validate/compile individual
    /// groups so a rejected group does not discard independently valid groups.
    pub fn validate_structure(&self) -> Result<()> {
        if self.groups.is_empty() {
            return Err(Error::invalid("structured proposal needs groups"));
        }
        let mut previous = BTreeSet::new();
        let mut slots = BTreeSet::new();
        for group in &self.groups {
            short(&group.key, "proposal group key")?;
            if group.depends_on.iter().any(|key| !previous.contains(key))
                || !previous.insert(group.key.clone())
            {
                return Err(Error::invalid(
                    "groups must have unique keys and topologically earlier dependencies",
                ));
            }
            for change in &group.changes {
                if !slots.insert(change.slot_key.clone()) {
                    return Err(Error::invalid(
                        "one proposal may write each target slot once",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_structure()?;
        for group in &self.groups {
            group.validate()?;
        }
        Ok(())
    }
    pub fn validate_against(
        &self,
        slots: &[StructuredTargetSlot],
        catalog: &[EvidenceSource],
    ) -> Result<()> {
        self.validate()?;
        let mut bound = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for slot in slots {
            slot.validate()?;
            if bound.insert(slot.key.clone(), slot).is_some() || !ids.insert(slot.id.clone()) {
                return Err(Error::invalid(
                    "target slots require unique keys and object IDs",
                ));
            }
        }
        let mut sources = BTreeSet::new();
        for source in catalog {
            source.validate()?;
            sources.insert(source.clone());
        }
        let owners = self
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .changes
                    .iter()
                    .map(move |change| (&change.slot_key, &group.key))
            })
            .collect::<BTreeMap<_, _>>();
        let mut ancestors = BTreeMap::<String, BTreeSet<String>>::new();
        for group in &self.groups {
            let mut allowed_groups = BTreeSet::from([group.key.clone()]);
            for dependency in &group.depends_on {
                allowed_groups.insert(dependency.clone());
                if let Some(prior) = ancestors.get(dependency) {
                    allowed_groups.extend(prior.iter().cloned());
                }
            }
            for change in &group.changes {
                let slot = bound
                    .get(&change.slot_key)
                    .ok_or_else(|| Error::invalid("proposal targets an undeclared slot"))?;
                if slot.kind != change.body.kind() {
                    return Err(Error::invalid(
                        "proposal content kind differs from target slot",
                    ));
                }
                reject_self_reference(&change.body, &change.evidence, &slot.id)?;
                for source in change.sources() {
                    if sources.contains(&source) {
                        continue;
                    }
                    let target = source.cognitive_ref().and_then(|reference| {
                        bound
                            .values()
                            .find(|target| {
                                target.output_reference().ok().as_ref() == Some(&reference)
                            })
                            .copied()
                    });
                    let permitted = target.is_some_and(|target| {
                        owners
                            .get(&target.key)
                            .is_some_and(|owner| allowed_groups.contains(*owner))
                    });
                    if !permitted {
                        return Err(Error::invalid(
                            "proposal source lies outside catalog or declared output dependencies",
                        ));
                    }
                }
            }
            ancestors.insert(group.key.clone(), allowed_groups);
        }
        Ok(())
    }
}
