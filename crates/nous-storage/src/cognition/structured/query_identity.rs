use super::read::decode;
use crate::{ReadView, ReferenceViewOptions, not_found, storage_error};
use nous_domain::*;
use std::collections::{BTreeMap, BTreeSet};

impl ReadView {
    pub async fn resolve_query_identity(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        kind: CognitiveKind,
        locator: &str,
        options: &ReferenceViewOptions<'_>,
    ) -> Result<ResolvedQueryIdentity> {
        if !matches!(
            kind,
            CognitiveKind::Concept | CognitiveKind::Entity | CognitiveKind::Schema
        ) {
            return Err(Error::invalid(
                "query identity must be Entity, Concept or Schema",
            ));
        }
        let objects = self
            .identity_snapshot(namespace, subject, options.as_of, false)
            .await?;
        let by_id = objects
            .iter()
            .map(|object| (object.id.as_str(), object))
            .collect::<BTreeMap<_, _>>();
        let addressed = if locator
            .parse::<nous_domain::references::LexicalRef>()
            .is_ok()
        {
            Some(
                self.resolve_reference_locator(namespace, subject, locator, options)
                    .await?,
            )
        } else {
            None
        };
        let exact = match addressed {
            Some(RetrievalTarget::Cognition(reference)) => Some(reference),
            Some(_) => {
                return Err(Error::invalid(
                    "query identity reference has a different type",
                ));
            }
            None => locator.parse::<CognitiveRef>().ok(),
        };
        if exact
            .as_ref()
            .is_some_and(|reference| reference.kind != kind)
        {
            return Err(Error::invalid("identity reference has a different kind"));
        }
        let key = locator.trim().to_lowercase();
        let mut candidates = BTreeMap::new();
        for object in &objects {
            if object.kind() != kind || object.deleted {
                continue;
            }
            let (label, aliases, _) = descriptor(object)?;
            let matches = exact.as_ref().map_or_else(
                || {
                    object.id.as_str() == locator
                        || label.trim().to_lowercase() == key
                        || aliases
                            .iter()
                            .any(|alias| alias.trim().to_lowercase() == key)
                },
                |reference| object.reference() == *reference,
            );
            if !matches {
                continue;
            }
            let mut resolved = object;
            let mut visited = BTreeSet::new();
            while let CognitiveContent::Concept(content) = &resolved.body {
                let Some(canonical) = &content.canonical else {
                    break;
                };
                if !visited.insert(resolved.id.as_str()) {
                    return Err(Error::invalid("Concept identity redirect contains a cycle"));
                }
                resolved = by_id
                    .get(canonical.id.as_str())
                    .copied()
                    .ok_or_else(not_found)?;
                if resolved.deleted || resolved.kind() != CognitiveKind::Concept {
                    return Err(not_found());
                }
            }
            candidates.insert(resolved.reference(), resolved);
        }
        if candidates.is_empty() {
            return Err(not_found());
        }
        if candidates.len() != 1 {
            let candidates = candidates
                .values()
                .map(|value| {
                    let (label, _, _) = descriptor(value)?;
                    Ok(serde_json::json!({"reference":value.reference(),"label":label}))
                })
                .collect::<Result<Vec<_>>>()?;
            return Err(
                Error::new(ErrorCode::Conflict, "query identity name is ambiguous")
                    .with_context("locator", locator)
                    .with_context("candidates", serde_json::json!(candidates)),
            );
        }
        let object = candidates.into_values().next().expect("one identity");
        let (label, aliases, description) = descriptor(object)?;
        let readable = if let Some(source_read) = options.source_read {
            self.source_read_allowed(source_read, &RetrievalTarget::Cognition(object.reference()))
                .await?
        } else {
            true
        };
        Ok(ResolvedQueryIdentity {
            reference: object.reference(),
            label: label.into(),
            aliases: aliases.to_vec(),
            description: readable.then(|| description.to_owned()),
        })
    }

    pub async fn resolve_entity_identifier(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        system: &str,
        key: &str,
        as_of: Option<i64>,
    ) -> Result<Vec<CognitiveRef>> {
        let objects = self
            .identity_snapshot(namespace, subject, as_of, false)
            .await?;
        Ok(objects
            .into_iter()
            .filter(|object| !object.deleted)
            .filter_map(|object| {
                let CognitiveContent::Entity(content) = &object.body else {
                    return None;
                };
                content
                    .identifiers
                    .iter()
                    .any(|identity| identity.system == system && identity.key == key)
                    .then(|| object.reference())
            })
            .collect())
    }

    pub async fn query_identity_targets(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        identities: &[CognitiveRef],
        as_of: Option<i64>,
        history: bool,
    ) -> Result<BTreeSet<RetrievalTarget>> {
        let objects = self
            .identity_snapshot(namespace, subject, as_of, history)
            .await?;
        let mut intersection: Option<BTreeSet<RetrievalTarget>> = None;
        for identity in identities {
            identity.validate()?;
            if !matches!(
                identity.kind,
                CognitiveKind::Entity | CognitiveKind::Concept | CognitiveKind::Schema
            ) {
                return Err(Error::invalid(
                    "query identity targets need Entity, Concept or Schema",
                ));
            }
            let object = self
                .structured_revision(
                    namespace,
                    &CognitiveId(identity.id.clone()),
                    identity.revision,
                )
                .await?;
            if object.subject_id != *subject
                || object.kind() != identity.kind
                || object.commit_seq > as_of.unwrap_or(self.watermark)
            {
                return Err(not_found());
            }
            let mut targets = BTreeSet::from([RetrievalTarget::Cognition(identity.clone())]);
            for object in objects.iter().filter(|object| !object.deleted) {
                let own = object.id.as_str() == identity.id && object.kind() == identity.kind;
                match &object.body {
                    CognitiveContent::Concept(content) if own => {
                        for target in &content.members {
                            targets.extend(
                                self.resolve_semantic_target(
                                    namespace, subject, target, as_of, history,
                                )
                                .await?,
                            );
                        }
                    }
                    CognitiveContent::Entity(content) if own => {
                        for mention in &content.mentions {
                            targets.extend(
                                self.resolve_semantic_target(
                                    namespace,
                                    subject,
                                    &mention.target,
                                    as_of,
                                    history,
                                )
                                .await?,
                            );
                        }
                    }
                    CognitiveContent::Schema(content) if own => targets.extend(
                        content
                            .support
                            .iter()
                            .filter_map(EvidenceSource::retrieval_target),
                    ),
                    CognitiveContent::MemorySemantics(content)
                        if content
                            .concepts
                            .iter()
                            .any(|reference| same_identity(reference, identity)) =>
                    {
                        targets.insert(RetrievalTarget::Cognition(CognitiveRef::memory(
                            &content.memory_id,
                            content.memory_revision,
                        )));
                    }
                    CognitiveContent::Schema(content)
                        if content
                            .scope
                            .concepts
                            .iter()
                            .chain(&content.scope.about)
                            .any(|reference| same_identity(reference, identity)) =>
                    {
                        targets.insert(RetrievalTarget::Cognition(object.reference()));
                    }
                    CognitiveContent::Association(content)
                        if content.polarity == AssociationPolarity::Positive =>
                    {
                        if same_identity(&content.from, identity) {
                            targets.insert(RetrievalTarget::Cognition(content.to.clone()));
                        }
                        if same_identity(&content.to, identity) {
                            targets.insert(RetrievalTarget::Cognition(content.from.clone()));
                        }
                    }
                    _ => {}
                }
            }
            intersection = Some(match intersection {
                None => targets,
                Some(previous) => previous.intersection(&targets).cloned().collect(),
            });
        }
        Ok(intersection.unwrap_or_default())
    }

    pub async fn resolve_semantic_target(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &SemanticTarget,
        as_of: Option<i64>,
        history: bool,
    ) -> Result<Vec<RetrievalTarget>> {
        target.validate()?;
        let cutoff = as_of.unwrap_or(self.watermark);
        if cutoff < 0 || cutoff > self.watermark {
            return Err(Error::invalid(
                "semantic relationship cutoff is outside committed authority",
            ));
        }
        let SemanticTarget::Object { identity } = target else {
            let SemanticTarget::Exact { reference } = target else {
                unreachable!()
            };
            return Ok(vec![reference.clone()]);
        };
        let revisions: Vec<i64> = if identity.kind == CognitiveKind::Memory {
            sqlx::query_scalar("SELECT revision FROM memory_revisions r WHERE namespace_id=? AND subject_id=? AND memory_id=? AND commit_seq<=? AND (? OR revision=(SELECT max(revision) FROM memory_revisions h WHERE h.memory_id=r.memory_id AND h.commit_seq<=?)) ORDER BY revision")
                .bind(namespace.as_str()).bind(subject.as_str()).bind(&identity.id).bind(cutoff).bind(history).bind(cutoff)
                .fetch_all(&mut *self.tx).await.map_err(storage_error)?
        } else {
            sqlx::query_scalar("SELECT revision FROM structured_revisions r WHERE namespace_id=? AND subject_id=? AND object_id=? AND kind=? AND commit_seq<=? AND (? OR revision=(SELECT max(revision) FROM structured_revisions h WHERE h.object_id=r.object_id AND h.commit_seq<=?)) ORDER BY revision")
                .bind(namespace.as_str()).bind(subject.as_str()).bind(&identity.id).bind(crate::codec::enum_name(&identity.kind)?).bind(cutoff).bind(history).bind(cutoff)
                .fetch_all(&mut *self.tx).await.map_err(storage_error)?
        };
        Ok(revisions
            .into_iter()
            .map(|revision| {
                RetrievalTarget::Cognition(CognitiveRef {
                    kind: identity.kind,
                    id: identity.id.clone(),
                    revision,
                })
            })
            .collect())
    }

    async fn identity_snapshot(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        as_of: Option<i64>,
        history: bool,
    ) -> Result<Vec<StructuredObject>> {
        self.subject(namespace, subject).await?;
        let cutoff = as_of.unwrap_or(self.watermark);
        if cutoff < 0 || cutoff > self.watermark {
            return Err(Error::invalid(
                "identity cutoff is outside committed authority",
            ));
        }
        let rows = sqlx::query("SELECT r.* FROM structured_revisions r WHERE r.namespace_id=? AND r.subject_id=? AND r.commit_seq<=? AND (? OR r.revision=(SELECT max(h.revision) FROM structured_revisions h WHERE h.object_id=r.object_id AND h.commit_seq<=?)) ORDER BY r.object_id,r.revision")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(cutoff).bind(history).bind(cutoff)
            .fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut objects = Vec::with_capacity(rows.len());
        for row in rows {
            objects.push(decode(&mut self.tx, row).await?);
        }
        Ok(objects)
    }
}

fn descriptor(object: &StructuredObject) -> Result<(&str, &[String], &str)> {
    Ok(match &object.body {
        CognitiveContent::Concept(content) => {
            (&content.label, &content.aliases, &content.description)
        }
        CognitiveContent::Entity(content) => {
            (&content.label, &content.aliases, &content.description)
        }
        CognitiveContent::Schema(content) => (&content.title, &[], &content.claim),
        _ => return Err(Error::invalid("object is not a named query identity")),
    })
}
fn same_identity(left: &CognitiveRef, right: &CognitiveRef) -> bool {
    left.kind == right.kind && left.id == right.id
}
