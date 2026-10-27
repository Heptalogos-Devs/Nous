use super::*;
use std::collections::BTreeSet;
impl SubjectPackage {
    pub fn addressable_targets(&self) -> BTreeSet<SemanticTarget> {
        let mut exacts = self
            .memories
            .iter()
            .map(|value| {
                RetrievalTarget::Cognition(CognitiveRef::memory(&value.id, value.revision))
            })
            .chain(
                self.cognition
                    .iter()
                    .map(|value| RetrievalTarget::Cognition(value.reference())),
            )
            .chain(self.materials.iter().map(|value| {
                RetrievalTarget::Media(MediaReference::Original {
                    material_id: value.material.id.clone(),
                })
            }))
            .chain(self.source_regions.iter().map(|value| {
                RetrievalTarget::Media(MediaReference::SourceRegion {
                    id: value.id.clone(),
                })
            }))
            .chain(self.representations.iter().map(|value| {
                RetrievalTarget::Media(MediaReference::Representation {
                    reference: value.reference(),
                })
            }))
            .chain(self.derived_regions.iter().map(|value| {
                RetrievalTarget::Media(MediaReference::DerivedRegion {
                    id: value.id.clone(),
                })
            }))
            .chain(self.tombstones.iter().map(|value| value.target.clone()))
            .collect::<BTreeSet<_>>();
        let mut result = BTreeSet::new();
        for reference in std::mem::take(&mut exacts) {
            if matches!(reference, RetrievalTarget::Cognition(_)) {
                result.insert(SemanticTarget::from_reference(reference.clone()));
            }
            result.insert(SemanticTarget::Exact { reference });
        }
        result
    }
    pub(crate) fn validate_addresses(&self) -> Result<()> {
        let allowed = self.addressable_targets();
        let mut seen = BTreeSet::new();
        for binding in &self.addresses {
            binding.validate()?;
            if !allowed.contains(&binding.target) || !seen.insert(binding) {
                return Err(Error::invalid(
                    "packaged reference address is duplicated or outside its exported identities",
                ));
            }
        }
        Ok(())
    }
}
