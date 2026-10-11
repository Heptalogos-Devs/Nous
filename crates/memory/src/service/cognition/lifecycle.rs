//! Cognition lifecycle transition and purge transactions.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl CognitionAction {
    fn canonical(self) -> String {
        match self {
            Self::Suppress => "suppress".into(),
            Self::Restore => "restore".into(),
            Self::Withdraw => "withdraw".into(),
            Self::Reaccept => "reaccept".into(),
            Self::Purge => "purge".into(),
            Self::SetAccessibility(mode) => format!(
                "set_accessibility:{}",
                match mode {
                    AccessibilityMode::Auto => "auto",
                    AccessibilityMode::Normal => "normal",
                    AccessibilityMode::Deep => "deep",
                    AccessibilityMode::Explicit => "explicit",
                }
            ),
        }
    }

    fn transition(self) -> Option<(&'static str, &'static str, &'static str)> {
        match self {
            Self::Suppress => Some(("suppression_state", "normal", "suppressed")),
            Self::Restore => Some(("suppression_state", "suppressed", "normal")),
            Self::Withdraw => Some(("acceptance_state", "accepted", "withdrawn")),
            Self::Reaccept => Some(("acceptance_state", "withdrawn", "accepted")),
            Self::Purge | Self::SetAccessibility(_) => None,
        }
    }
}

impl MemoryService {
    /// Persist the authority-side purge checkpoint before Kernel reclaims
    /// prior-generation Serving artifacts.
    pub async fn begin_purge_cognition(
        &self,
        subject: SubjectId,
        target: CognitiveRef,
        operation: OperationId,
        expected_epoch: i64,
    ) -> Result<()> {
        let (kind, object_id, exact) = CognitionKind::from_reference(&target)?;
        if exact {
            return Err(Error::new(
                cognition_owner(kind),
                ErrorCode::InvalidRequest,
                "purge target must be an object reference",
            ));
        }
        let owner = cognition_owner(kind);
        let digest = operation_digest(
            "cognition_lifecycle",
            subject,
            &serde_json::json!({
                "target": {"kind": kind.as_str(), "id": object_id},
                "action": "purge",
                "expected_object_epoch": expected_epoch,
            }),
        )?;
        let mut mutation = match self
            .resume_mutation(owner, subject, operation, "cognition_lifecycle", &digest)
            .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT object_epoch,purge_state FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 FOR UPDATE")
            .bind(subject.0)
            .bind(kind.as_str())
            .bind(object_id)
            .fetch_optional(&mut **mutation.tx())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::new(owner, ErrorCode::NotFound, "Cognition object not found"))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        let purge: String = row.try_get("purge_state").map_err(db)?;
        if purge == "purging" {
            let checkpoint =
                MutationResult::new(kind.object_ref(object_id), None, Some(expected_epoch + 1));
            if epoch == expected_epoch + 1 && mutation.checkpoint_matches(&checkpoint).await? {
                return mutation.checkpoint().await;
            }
            return Err(Error::new(
                owner,
                ErrorCode::OperationInProgress,
                "Cognition purge is already in progress",
            ));
        }
        fence_epoch(owner, epoch, expected_epoch)?;

        let revision_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT revision_id FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3",
        )
        .bind(subject.0)
        .bind(kind.as_str())
        .bind(object_id)
        .fetch_all(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let identity_refs = std::iter::once(kind.object_ref(object_id))
            .chain(revision_ids.iter().copied().map(|id| kind.revision_ref(id)))
            .collect::<Vec<_>>();
        let revision_refs = revision_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        sqlx::query("UPDATE cognition_objects SET purge_state='purging',object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3")
            .bind(subject.0)
            .bind(kind.as_str())
            .bind(object_id)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        let invalidation = if kind == CognitionKind::Journal {
            ProjectionInvalidation::text()
        } else {
            ProjectionInvalidation::all()
        };
        let sequence = mutation.invalidate(invalidation).await?;
        if kind == CognitionKind::Journal {
            self.wake_journal_revalidation_in(
                mutation.tx(),
                subject,
                nous_core::JournalId(object_id),
                sequence,
            )
            .await?;
        }
        let dependency_kind = if kind == CognitionKind::CognitiveSchema {
            "schema"
        } else {
            kind.as_str()
        };
        let reason = if matches!(kind, CognitionKind::Memory | CognitionKind::CognitiveSchema) {
            "basis_purged"
        } else {
            "source_purged"
        };
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            dependency_kind,
            &[object_id],
            sequence,
            reason,
        )
        .await?;
        AuthorityStore::advance_serving_purge_epoch_in(mutation.tx(), subject).await?;
        AuthorityStore::tombstone_identity_refs_in(mutation.tx(), subject, &identity_refs).await?;

        AuthorityStore::purge_work_contexts_in(mutation.tx(), subject, &identity_refs).await?;
        AuthorityStore::purge_embedding_material_uses_in(mutation.tx(), subject, &identity_refs)
            .await?;
        AuthorityStore::purge_owner_history_in(mutation.tx(), subject, &identity_refs).await?;

        let revision_kind = format!("{}_revision", kind.as_str());
        let now = self.cognition.now(subject);
        sqlx::query("DELETE FROM resident_refs WHERE ref_kind=$1 AND ref_value=ANY($2::text[])")
            .bind(&revision_kind)
            .bind(&revision_refs)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        sqlx::query("UPDATE cognitive_schema_evidence_links SET revoked_at=COALESCE(revoked_at,$4) WHERE subject_id=$1 AND basis_kind=$2 AND basis_ref=ANY($3::text[]) AND revoked_at IS NULL")
            .bind(subject.0)
            .bind(&revision_kind)
            .bind(&revision_refs)
            .bind(now)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT subject_id,consumer_ref,event_id,request_digest,$4 FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind=$2 AND ref_value=ANY($3::text[]) ON CONFLICT DO NOTHING")
            .bind(subject.0)
            .bind(&revision_kind)
            .bind(&revision_refs)
            .bind(now)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind=$2 AND ref_value=ANY($3::text[])")
            .bind(subject.0)
            .bind(&revision_kind)
            .bind(&revision_refs)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        sqlx::query("DELETE FROM association_evidence WHERE subject_id=$1 AND ((from_ref_kind=$2 AND from_ref=ANY($3::text[])) OR (to_ref_kind=$2 AND to_ref=ANY($3::text[])) OR association_evidence_id IN (SELECT association_evidence_id FROM association_evidence_basis WHERE basis_kind=$2 AND basis_ref=ANY($3::text[])))")
            .bind(subject.0)
            .bind(&revision_kind)
            .bind(&revision_refs)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        mutation
            .checkpoint_with_result(&MutationResult::new(
                kind.object_ref(object_id),
                None,
                Some(expected_epoch + 1),
            ))
            .await
    }

    /// Remove the authority header after all old Serving generations are gone.
    pub async fn complete_purge_cognition(
        &self,
        subject: SubjectId,
        target: CognitiveRef,
        operation: OperationId,
        expected_epoch: i64,
    ) -> Result<()> {
        let (kind, object_id, exact) = CognitionKind::from_reference(&target)?;
        if exact {
            return Err(Error::new(
                cognition_owner(kind),
                ErrorCode::InvalidRequest,
                "purge target must be an object reference",
            ));
        }
        let owner = cognition_owner(kind);
        let digest = operation_digest(
            "cognition_lifecycle",
            subject,
            &serde_json::json!({
                "target": {"kind": kind.as_str(), "id": object_id},
                "action": "purge",
                "expected_object_epoch": expected_epoch,
            }),
        )?;
        let mut mutation = match MutationEnvelope::resume(
            &self.store,
            subject,
            operation,
            "cognition_lifecycle",
            &digest,
            Some(OwnerLock("memory-authority")),
            owner,
        )
        .await?
        {
            MutationStart::Replay(_) => return Ok(()),
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT object_epoch,purge_state FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 FOR UPDATE")
            .bind(subject.0)
            .bind(kind.as_str())
            .bind(object_id)
            .fetch_optional(&mut **mutation.tx())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::new(owner, ErrorCode::NotFound, "Cognition object not found"))?;
        if row.try_get::<String, _>("purge_state").map_err(db)? != "purging"
            || row.try_get::<i64, _>("object_epoch").map_err(db)? != expected_epoch + 1
        {
            return Err(Error::new(
                owner,
                ErrorCode::FailedPrecondition,
                "Cognition purge checkpoint is missing or changed",
            ));
        }
        if !mutation
            .checkpoint_matches(&MutationResult::new(
                kind.object_ref(object_id),
                None,
                Some(expected_epoch + 1),
            ))
            .await?
        {
            return Err(Error::new(
                owner,
                ErrorCode::OperationInProgress,
                "Cognition purge checkpoint belongs to another operation",
            ));
        }
        sqlx::query("SELECT subject_id FROM subjects WHERE subject_id=$1 FOR UPDATE")
            .bind(subject.0)
            .fetch_one(&mut **mutation.tx())
            .await
            .map_err(db)?;
        if AuthorityStore::has_unreclaimed_serving_artifact_in(mutation.tx(), subject).await? {
            return Err(Error::new(
                owner,
                ErrorCode::OperationInProgress,
                "Serving purge reclamation remains in progress",
            ));
        }
        sqlx::query(
            "DELETE FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3",
        )
        .bind(subject.0)
        .bind(kind.as_str())
        .bind(object_id)
        .execute(&mut **mutation.tx())
        .await
        .map_err(db)?;
        mutation
            .commit(&[MutationResult::new(
                kind.object_ref(object_id),
                None,
                Some(expected_epoch + 1),
            )])
            .await
    }

    /// Apply shared status/accessibility transitions using one receipt and one
    /// Cognition header lock. Family-specific checks and invalidation policies
    /// remain explicit below.
    pub async fn mutate_cognition(
        &self,
        subject: SubjectId,
        target: CognitiveRef,
        operation: OperationId,
        expected_epoch: i64,
        action: CognitionAction,
    ) -> Result<CognitionView> {
        let (kind, object_id, exact) = CognitionKind::from_reference(&target)?;
        if exact {
            return Err(Error::new(
                cognition_owner(kind),
                ErrorCode::InvalidRequest,
                "lifecycle target must be an object reference",
            ));
        }
        if matches!(action, CognitionAction::SetAccessibility(_)) && kind != CognitionKind::Memory {
            return Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::InvalidRequest,
                "accessibility is defined only for Memory",
            ));
        }
        if action == CognitionAction::Purge {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "purge must use the Serving-coordinated purge path",
            ));
        }
        let error_owner = cognition_owner(kind);
        let action_name = action.canonical();
        let digest = operation_digest(
            "cognition_lifecycle",
            subject,
            &serde_json::json!({
                "target": {"kind": kind.as_str(), "id": object_id},
                "action": action_name,
                "expected_object_epoch": expected_epoch,
            }),
        )?;
        let mut mutation = match self
            .start_mutation(
                error_owner,
                subject,
                operation,
                "cognition_lifecycle",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                let target = kind.object_ref(object_id);
                let revision = receipt
                    .results
                    .iter()
                    .find_map(|result| {
                        (result.reference == target)
                            .then_some(result.revision_id)
                            .flatten()
                    })
                    .ok_or_else(|| {
                        Error::new(
                            error_owner,
                            ErrorCode::InternalFailure,
                            "committed Cognition lifecycle receipt has no selected revision",
                        )
                    })?;
                return self
                    .read_cognition(subject, kind.revision_ref(revision))
                    .await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT * FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 FOR UPDATE")
            .bind(subject.0).bind(kind.as_str()).bind(object_id)
            .fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::new(error_owner, ErrorCode::NotFound, "Cognition object not found"))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        fence_epoch(error_owner, epoch, expected_epoch)?;
        let purge: String = row.try_get("purge_state").map_err(db)?;
        require_transition(error_owner, &purge, "normal")?;

        let mut next_epoch = epoch;
        let result_revision: Uuid = row.try_get("current_revision_id").map_err(db)?;
        let mut changed = true;
        if let Some((field, from, to)) = action.transition() {
            let current: String = row.try_get(field).map_err(db)?;
            if current == to {
                changed = false;
            } else {
                require_transition(error_owner, &current, from)?;
                if kind == CognitionKind::Episode && field == "acceptance_state" && to == "accepted"
                {
                    super::episode::validate_episode_reaccept_in(
                        mutation.tx(),
                        subject,
                        nous_core::EpisodeId(object_id),
                    )
                    .await?;
                }
                let statement = match field {
                    "suppression_state" => {
                        "UPDATE cognition_objects SET suppression_state=$4,object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3"
                    }
                    "acceptance_state" => {
                        "UPDATE cognition_objects SET acceptance_state=$4,object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3"
                    }
                    _ => unreachable!(),
                };
                sqlx::query(statement)
                    .bind(subject.0)
                    .bind(kind.as_str())
                    .bind(object_id)
                    .bind(to)
                    .execute(&mut **mutation.tx())
                    .await
                    .map_err(db)?;
            }
        } else if let CognitionAction::SetAccessibility(mode) = action {
            let value = match mode {
                AccessibilityMode::Auto => "auto",
                AccessibilityMode::Normal => "normal",
                AccessibilityMode::Deep => "deep",
                AccessibilityMode::Explicit => "explicit",
            };
            let current: String = row.try_get("accessibility_mode").map_err(db)?;
            if current == value {
                changed = false;
            } else {
                sqlx::query("UPDATE cognition_objects SET accessibility_mode=$4,object_epoch=object_epoch+1 WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3")
                    .bind(subject.0).bind(kind.as_str()).bind(object_id).bind(value).execute(&mut **mutation.tx()).await.map_err(db)?;
            }
        }

        if changed {
            next_epoch = epoch + 1;
            if matches!(kind, CognitionKind::Memory | CognitionKind::CognitiveSchema) {
                let revision_kind = if kind == CognitionKind::Memory {
                    "memory_revision"
                } else {
                    "cognitive_schema_revision"
                };
                sqlx::query("DELETE FROM resident_refs WHERE ref_kind=$4 AND ref_value IN (SELECT revision_id::text FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3)")
                    .bind(subject.0).bind(kind.as_str()).bind(object_id).bind(revision_kind)
                    .execute(&mut **mutation.tx()).await.map_err(db)?;
            }
            let invalidation = if kind == CognitionKind::Journal {
                ProjectionInvalidation::text()
            } else {
                ProjectionInvalidation::all()
            };
            let sequence = mutation.invalidate(invalidation).await?;
            if kind == CognitionKind::Journal {
                self.wake_journal_revalidation_in(
                    mutation.tx(),
                    subject,
                    nous_core::JournalId(object_id),
                    sequence,
                )
                .await?;
            }
            if !matches!(action, CognitionAction::SetAccessibility(_)) {
                let dependency_kind = match kind {
                    CognitionKind::Memory => "memory",
                    CognitionKind::CognitiveSchema => "schema",
                    CognitionKind::Episode => "episode",
                    CognitionKind::Journal => "journal",
                };
                self.invalidate_object_dependents_in(
                    mutation.tx(),
                    subject,
                    dependency_kind,
                    &[object_id],
                    sequence,
                    "source_lifecycle_changed",
                )
                .await?;
            }
        }
        mutation
            .commit(&[MutationResult::revised(
                kind.object_ref(object_id),
                result_revision,
                Some(next_epoch),
            )])
            .await?;
        self.read_cognition(subject, kind.revision_ref(result_revision))
            .await
    }
}
