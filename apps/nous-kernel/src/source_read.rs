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
    async fn read_scope_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
        scope: &SourceReadScope,
    ) -> Result<SourceReadView> {
        self.view_in(tx, subject, scope).await
    }

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
                    owner.source_references_in(tx, subject, &reference).await?
                }
                CognitiveDomain::SelfCognition => {
                    let owner = self
                        .self_cognition
                        .as_ref()
                        .ok_or_else(|| Error::Unavailable("Self unavailable".into()))?;
                    owner.source_references_in(tx, subject, &reference).await?
                }
                CognitiveDomain::Motivation => {
                    let rows = nous_persistence::AuthorityStore::motivation_source_edges_in(
                        tx,
                        subject,
                        reference.uuid().expect("Motivation UUID"),
                    )
                    .await?;
                    rows.into_iter()
                        .map(|(k, v)| parse_reference(&k, &v))
                        .collect::<Result<Vec<_>>>()?
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

impl SourceRoots {
    pub async fn view_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
        scope: &SourceReadScope,
    ) -> Result<SourceReadView> {
        let mut view = SourceReadView {
            subject,
            ..Default::default()
        };
        if scope.subject_id.is_none()
            && scope.source_refs.is_empty()
            && scope.origin_conversation_refs.is_empty()
        {
            return Ok(view);
        }
        if scope.subject_id != Some(subject) {
            return Err(Error::Invalid("source read Subject mismatch".into()));
        }
        if scope.origin_conversation_refs.len() > 64
            || scope
                .origin_conversation_refs
                .iter()
                .any(|v| v.trim().is_empty() || v.len() > 1024)
        {
            return Err(Error::Invalid("invalid trusted origin source scope".into()));
        }
        view.origin_conversation_refs = scope.origin_conversation_refs.clone();
        let mut sources = Vec::new();
        for reference in &scope.source_refs {
            if !self
                .store
                .reference_in_subject_tx(tx, subject, reference)
                .await?
            {
                return Err(Error::Invalid(
                    "source read reference outside Subject".into(),
                ));
            }
            if let CognitiveRef::WorkContext(id) = reference {
                view.work_contexts.push(id.0);
            } else {
                sources.push(reference.clone());
            }
        }
        let roots = nous_runtime::SourceRootResolver::roots_in(self, tx, subject, &sources).await?;
        if roots
            .iter()
            .any(|root| root.certainty != EvidenceRootCertainty::Known)
        {
            return Err(Error::Unavailable(
                "unknown source authorization lineage".into(),
            ));
        }
        view.root_keys = roots.into_iter().map(|root| root.root_key).collect();
        let metadata =
            AuthorityStore::source_read_metadata_closure_in(tx, subject, &sources).await?;
        if metadata.len() > 1024 || metadata.iter().any(|v| v.2) {
            return Err(Error::Unavailable(
                "explicit source metadata closure exceeds budget or cycles".into(),
            ));
        }
        view.explicit_refs = metadata
            .into_iter()
            .map(|(kind, id, _)| serde_json::json!({"kind":kind,"id":id}))
            .collect();
        Ok(view)
    }
    pub async fn view(
        &self,
        subject: SubjectId,
        scope: &SourceReadScope,
    ) -> Result<SourceReadView> {
        let mut tx = self.store.read_view().await?;
        let view = self.view_in(&mut tx, subject, scope).await?;
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        Ok(view)
    }
}
