//! Secondary address allocation is part of the semantic owner's transaction.
mod packages;
mod reconcile;
mod validation;
use crate::{ReadView, WriteTransaction, storage_error, write_error};
use nous_domain::{references::*, *};
use sqlx::Row;
pub struct ReferenceViewOptions<'a> {
    pub as_of: Option<i64>,
    pub source_read: Option<&'a crate::SourceReadView>,
}

impl WriteTransaction {
    pub async fn ensure_reference_address(
        &mut self,
        namespace: &NamespaceId,
        target: &SemanticTarget,
    ) -> Result<LexicalRef> {
        target.validate()?;
        let encoded = serde_json::to_string(target)?;
        if let Some(value) = sqlx::query_scalar::<_, String>(
            "SELECT address FROM reference_addresses WHERE namespace_id=? AND target=?",
        )
        .bind(namespace.as_str())
        .bind(&encoded)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage_error)?
        {
            return value.parse();
        }
        for _ in 0..32 {
            let bytes = uuid::Uuid::new_v4().into_bytes();
            let bits = u64::from_be_bytes([
                0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
            ]);
            let candidate = LexicalRef::allocate(target, bits)?;
            if sqlx::query("INSERT INTO reference_addresses(address,namespace_id,target,recorded_at,commit_seq) VALUES(?,?,?,?,?) ON CONFLICT DO NOTHING")
                .bind(candidate.as_str()).bind(namespace.as_str()).bind(&encoded).bind(self.time().recorded()).bind(self.commit_seq())
                .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected()==1 {
                return Ok(candidate);
            }
        }
        Err(Error::new(
            ErrorCode::ResourceExhausted,
            "readable reference allocation exhausted its collision retry bound",
        ))
    }
    pub async fn ensure_retrieval_addresses(
        &mut self,
        namespace: &NamespaceId,
        reference: &RetrievalTarget,
    ) -> Result<ReferenceAddresses> {
        let object = if matches!(reference, RetrievalTarget::Cognition(_)) {
            Some(
                self.ensure_reference_address(
                    namespace,
                    &SemanticTarget::from_reference(reference.clone()),
                )
                .await?,
            )
        } else {
            None
        };
        let exact = self
            .ensure_reference_address(
                namespace,
                &SemanticTarget::Exact {
                    reference: reference.clone(),
                },
            )
            .await?;
        Ok(ReferenceAddresses { object, exact })
    }
}
impl ReadView {
    pub async fn resolve_reference_locator(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        locator: &str,
        options: &ReferenceViewOptions<'_>,
    ) -> Result<RetrievalTarget> {
        if let Ok(reference) = locator.parse::<RetrievalTarget>() {
            return Ok(reference);
        }
        let address = locator.parse::<LexicalRef>()?;
        let encoded = self
            .reference_bindings_at(namespace, subject, &address, options.as_of)
            .await?;
        let mut candidates = std::collections::BTreeSet::new();
        let mut tombstoned = false;
        let filters = QueryFilters::default();
        let snapshot = crate::QuerySnapshotOptions {
            space_id: "",
            as_of_commit: options.as_of,
            limit: 1,
            source_read: options.source_read,
            filters: &filters,
            history: false,
        };
        for encoded in encoded {
            let target: SemanticTarget = serde_json::from_str(&encoded)?;
            let erased = match &target {
                SemanticTarget::Object { identity } => {
                    crate::source::tombstone::known_identity(
                        &mut self.tx,
                        namespace,
                        subject,
                        identity,
                    )
                    .await?
                }
                SemanticTarget::Exact { reference } => {
                    crate::source::tombstone::read(&mut self.tx, namespace, subject, reference)
                        .await?
                        .is_some()
                }
            };
            if erased {
                tombstoned = true;
                continue;
            }
            for reference in self
                .resolve_semantic_target(namespace, subject, &target, options.as_of, false)
                .await?
            {
                match self
                    .require_retrieval_reference(namespace, subject, &snapshot, &reference)
                    .await
                {
                    Ok(()) => {
                        candidates.insert(reference);
                    }
                    Err(error)
                        if matches!(error.code, ErrorCode::NotFound | ErrorCode::Forbidden) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        if candidates.len() > 1 {
            return Err(Error::new(
                ErrorCode::Conflict,
                "readable reference has multiple eligible imported targets",
            )
            .with_context("problem", "query.ambiguous_reference")
            .with_context("candidates", serde_json::to_value(candidates)?));
        }
        candidates.into_iter().next().ok_or_else(|| {
            crate::not_found().with_context(
                "problem",
                if tombstoned {
                    "query.reference_tombstoned"
                } else {
                    "query.unknown_reference"
                },
            )
        })
    }
    async fn reference_bindings_at(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        address: &LexicalRef,
        as_of: Option<i64>,
    ) -> Result<Vec<String>> {
        let cut = as_of.unwrap_or(self.watermark);
        if cut < 0 || cut > self.watermark {
            return Err(Error::invalid(
                "reference Authority cut is outside committed history",
            ));
        }
        let mut targets: Vec<String> = sqlx::query_scalar(
            "SELECT target FROM reference_addresses WHERE namespace_id=? AND address=?",
        )
        .bind(namespace.as_str())
        .bind(address.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        let rows=sqlx::query("SELECT r.target,b.uninstalled_at FROM imported_reference_addresses r JOIN bundle_installations b ON b.id=r.installation_id WHERE b.namespace_id=? AND b.subject_id=? AND r.address=? AND b.commit_seq<=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(address.as_str()).bind(cut).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            if let Some(recorded) = row.get::<Option<&str>, _>("uninstalled_at")
                && (as_of.is_none()
                    || self
                        .commit_at_recorded(&OperationTime::parse(recorded)?)
                        .await?
                        <= cut)
            {
                continue;
            }
            targets.push(row.get("target"));
        }
        Ok(targets)
    }
    pub async fn retrieval_addresses(
        &mut self,
        namespace: &NamespaceId,
        reference: &RetrievalTarget,
    ) -> Result<Option<ReferenceAddresses>> {
        let exact = self
            .reference_address(
                namespace,
                &SemanticTarget::Exact {
                    reference: reference.clone(),
                },
            )
            .await?;
        let Some(exact) = exact else { return Ok(None) };
        let object = if matches!(reference, RetrievalTarget::Cognition(_)) {
            self.reference_address(
                namespace,
                &SemanticTarget::from_reference(reference.clone()),
            )
            .await?
        } else {
            None
        };
        Ok(Some(ReferenceAddresses { object, exact }))
    }
    pub async fn reference_address(
        &mut self,
        namespace: &NamespaceId,
        target: &SemanticTarget,
    ) -> Result<Option<LexicalRef>> {
        sqlx::query_scalar::<_, String>(
            "SELECT address FROM reference_addresses WHERE namespace_id=? AND target=?",
        )
        .bind(namespace.as_str())
        .bind(serde_json::to_string(target)?)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage_error)?
        .map(|value| value.parse())
        .transpose()
    }
}
