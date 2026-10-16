// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use nous_core::*;
use nous_persistence::AuthorityStore;
use std::collections::{BTreeSet, HashSet, VecDeque};

#[derive(Clone)]
pub(crate) struct SourceRoots {
    pub store: AuthorityStore,
    pub material: nous_material::MaterialService,
    pub memory: Option<nous_memory::MemoryService>,
    pub self_cognition: Option<nous_self::SelfService>,
}
#[async_trait::async_trait]
impl nous_runtime::SourceRootResolver for SourceRoots {
    async fn roots_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<BTreeSet<EvidenceRoot>> {
        if references.len() > 1024 {
            return Err(Error::Invalid("source root budget exceeded".into()));
        }
        let mut pending = VecDeque::from(
            references
                .iter()
                .cloned()
                .map(|r| (r, Vec::<CognitiveRef>::new()))
                .collect::<Vec<_>>(),
        );
        let mut seen = HashSet::new();
        let mut roots = BTreeSet::new();
        while let Some((reference, mut path)) = pending.pop_front() {
            if path.contains(&reference) {
                return Err(Error::Invalid("source lineage cycle".into()));
            }
            path.push(reference.clone());
            if !seen.insert(reference.clone()) {
                continue;
            }
            if seen.len() > 1024 {
                return Err(Error::Unavailable(
                    "source lineage exceeds bounded read".into(),
                ));
            }
            if !self
                .store
                .reference_in_subject_tx(tx, subject, &reference)
                .await?
            {
                return Err(Error::Invalid(
                    "source read reference is outside Subject".into(),
                ));
            }
            if reference.domain() == CognitiveDomain::Material {
                let resolved = self
                    .material
                    .lineage_roots_batch_in(tx, subject, std::slice::from_ref(&reference))
                    .await?;
                if resolved.is_empty() {
                    return Err(Error::Unavailable("source has unknown lineage".into()));
                }
                roots.extend(resolved);
                continue;
            }
            if let CognitiveRef::CognitiveSeedVersion(id) = reference {
                pending.push_back((
                    AuthorityStore::seed_source_reference_in(tx, subject, id).await?,
                    path,
                ));
                continue;
            }
            if !reference.is_cognition_revision()
                || !self
                    .store
                    .cognition_available_read_in(tx, subject, &reference)
                    .await?
            {
                return Err(Error::Unavailable(
                    "source requires available exact cognition or Material".into(),
                ));
            }
            let children = match reference.domain() {
                CognitiveDomain::Memory => {
                    let owner = self
                        .memory
                        .as_ref()
                        .ok_or_else(|| Error::Unavailable("Memory unavailable".into()))?;
                    let nodes = owner
                        .trace_nodes_in(tx, subject, std::slice::from_ref(&reference))
                        .await?;
                    nodes
                        .into_iter()
                        .flat_map(|n| n.trace_sources().references)
                        .collect::<Vec<_>>()
                }
                CognitiveDomain::SelfCognition => {
                    let owner = self
                        .self_cognition
                        .as_ref()
                        .ok_or_else(|| Error::Unavailable("Self unavailable".into()))?;
                    owner
                        .trace_node_in(tx, subject, &reference)
                        .await?
                        .1
                        .references
                }
                CognitiveDomain::SocialCognition => {
                    let resolved = AuthorityStore::social_source_roots_in(
                        tx,
                        subject,
                        reference.uuid().expect("Social UUID"),
                    )
                    .await?;
                    if resolved.is_empty() {
                        return Err(Error::Unavailable("Social source lineage missing".into()));
                    }
                    roots.extend(resolved);
                    continue;
                }
                _ => return Err(Error::Invalid("unsupported source reference".into())),
            };
            if children.is_empty() {
                return Err(Error::Unavailable(
                    "cognition source lineage is unknown".into(),
                ));
            }
            pending.extend(children.into_iter().map(|r| (r, path.clone())));
        }
        Ok(roots)
    }
}
