//! memory_maintenance_service.py at 28db65dd96f546f736d65c043a39254cbae2c4e9.

use crate::App;
use nous_domain::*;
use nous_storage::operation_key;
use nous_storage::{BeginOperation, OperationReceipt};
use serde_json::{Value, json};

const HOUR_MS: i64 = 3_600_000;
const FREEZE_THRESHOLD: f64 = 0.1;
const REVIVE_THRESHOLD: f64 = 0.15;

impl App {
    pub async fn change_memory(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &MemoryId,
        change: MemoryChange,
    ) -> Result<OperationReceipt> {
        let payload = json!({"memory_id":id,"change":change});
        self.apply_memory_change(ctx, operation_id, id, change, "memory.change", payload)
            .await
    }

    pub(crate) async fn apply_memory_change(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &MemoryId,
        change: MemoryChange,
        kind: &str,
        payload: Value,
    ) -> Result<OperationReceipt> {
        validate_change(&change)?;
        let namespace = self.database.memory_namespace(id).await?;
        let existing = match self.database.memory(&namespace, id).await {
            Ok(memory) => Some(memory),
            Err(error) if error.code == ErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let subject_id = match &existing {
            Some(memory) => memory.subject_id.clone(),
            None => {
                self.database
                    .purged_memory(&namespace, id)
                    .await?
                    .subject_id
            }
        };
        ctx.require_subject(&namespace, &subject_id)?;
        if change.action == MemoryAction::Purge {
            ctx.require_namespace(&namespace)?;
        }
        let scope = format!("namespace:{namespace}/subject:{subject_id}");
        let key = operation_key(ctx, kind, &scope, operation_id, &payload)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut memory = tx.memory(&namespace, id).await?;
        if memory.revision != change.expected_revision {
            return Err(Error::new(ErrorCode::Conflict, "memory revision changed"));
        }
        let now = tx.time().unix_ms();
        let before = memory.revision;
        if change.action == MemoryAction::Purge {
            if !memory.deleted {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "purge requires a memory in the recycle bin",
                ));
            }
            if !change.force && now.saturating_sub(memory.times.recorded.coordinate) < 24 * HOUR_MS
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "purge grace period is 24 hours; namespace authority may explicitly force it",
                ));
            }
            let marker = tx.purge_memory(&namespace, id, before).await?;
            let mut event = audit(
                ctx,
                &memory,
                "purge",
                &change.reason,
                before,
                json!({"force":change.force}),
                tx.time(),
            );
            event.revision = marker.revision;
            event.commit_seq = marker.commit_seq;
            tx.append_memory_audit(&event).await?;
            return tx
                .commit(json!({"id":id,"purged":true,"revision":marker.revision}))
                .await;
        }
        apply_to_memory(&mut memory, &change, now)?;
        memory.revision += 1;
        memory.commit_seq = tx.commit_seq();
        memory.recorded_at = tx.time().recorded();
        memory.times.recorded = tx.time().point();
        tx.save_memory(&memory, Some(before)).await?;
        tx.append_memory_audit(&audit(
            ctx,
            &memory,
            &action_name(change.action),
            &change.reason,
            before,
            json!({"lifecycle":memory.lifecycle,"deleted":memory.deleted}),
            tx.time(),
        ))
        .await?;
        tx.commit(serde_json::to_value(memory)?).await
    }

    pub async fn recycle_bin(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        validate_limit(limit)?;
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        self.database.recycle_bin(&namespace, subject, limit).await
    }

    pub async fn memory_audit(
        &self,
        ctx: &CallContext,
        id: &MemoryId,
        limit: usize,
    ) -> Result<Vec<MemoryAudit>> {
        validate_limit(limit)?;
        let namespace = self.database.memory_namespace(id).await?;
        let subject = match self.database.memory(&namespace, id).await {
            Ok(memory) => memory.subject_id,
            Err(error) if error.code == ErrorCode::NotFound => {
                self.database
                    .purged_memory(&namespace, id)
                    .await?
                    .subject_id
            }
            Err(error) => return Err(error),
        };
        ctx.require_subject(&namespace, &subject)?;
        self.database.memory_audit(&namespace, id, limit).await
    }

    pub async fn maintain_subject(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject: &SubjectId,
        limit: usize,
        after: Option<MemoryId>,
        snapshot_seq: Option<i64>,
    ) -> Result<OperationReceipt> {
        validate_limit(limit)?;
        let namespace = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&namespace, subject)?;
        let page = self
            .database
            .maintenance_page(&namespace, subject, after.as_ref(), snapshot_seq, limit)
            .await?;
        let scope = format!("namespace:{namespace}/subject:{subject}");
        let key = operation_key(
            ctx,
            "memory.maintain",
            &scope,
            operation_id,
            &json!({"subject_id":subject,"limit":limit,"after":after,"snapshot_seq":snapshot_seq,"policy":"a-memorix.lifecycle"}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let now = tx.time().unix_ms();
        let mut changed = Vec::new();
        for candidate in &page.memories {
            let mut memory = match tx.memory(&namespace, &candidate.id).await {
                Ok(memory) => memory,
                Err(error) if error.code == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            if memory.deleted || memory.lifecycle.protected(now) {
                continue;
            }
            let old = memory.lifecycle.clone();
            let before = memory.revision;
            let archive = old.activation == MemoryActivation::Decayed
                && old
                    .inactive_since_unix_ms
                    .is_some_and(|at| now.saturating_sub(at) >= 24 * HOUR_MS)
                && retention(&old, now) <= FREEZE_THRESHOLD;
            if archive {
                memory.deleted = true;
            } else {
                memory.lifecycle = transition(&old, MemoryAction::Decay, now, 1.0, None)?;
            }
            if memory.lifecycle == old && !archive {
                continue;
            }
            memory.revision += 1;
            memory.commit_seq = tx.commit_seq();
            memory.recorded_at = tx.time().recorded();
            memory.times.recorded = tx.time().point();
            tx.save_memory(&memory, Some(before)).await?;
            tx.append_memory_audit(&audit(
                ctx,
                &memory,
                if archive { "auto_archive" } else { "decay" },
                "lifecycle evaluation",
                before,
                json!({"lifecycle":memory.lifecycle}),
                tx.time(),
            ))
            .await?;
            changed.push(memory.id);
        }
        tx.commit(json!({"subject_id":subject,"scanned":page.memories.len(),"changed":changed,"limit":limit,"next_cursor":page.next_cursor,"snapshot_seq":page.snapshot_seq,"complete":!page.has_more})).await
    }
}

fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=1000).contains(&limit) {
        Err(Error::invalid("limit must be 1..1000"))
    } else {
        Ok(())
    }
}
fn validate_change(change: &MemoryChange) -> Result<()> {
    if change.expected_revision < 1 || change.reason.len() > 4096 {
        return Err(Error::invalid(
            "expected_revision must be positive and reason at most 4096 bytes",
        ));
    }
    if change
        .strength
        .is_some_and(|strength| !strength.is_finite() || strength < 0.0)
        || change
            .hours
            .is_some_and(|hours| !hours.is_finite() || hours.abs() > 1_000_000.0)
    {
        return Err(Error::invalid(
            "strength and hours must be finite and within supported bounds",
        ));
    }
    if change.edit.is_some() && change.action != MemoryAction::Revise {
        return Err(Error::invalid("edit is only accepted for revise"));
    }
    Ok(())
}
fn apply_to_memory(memory: &mut Memory, change: &MemoryChange, now: i64) -> Result<()> {
    if memory.deleted && change.action != MemoryAction::Restore {
        return Err(Error::new(
            ErrorCode::Conflict,
            "restore the memory before changing it",
        ));
    }
    match change.action {
        MemoryAction::Trash => {
            if memory.lifecycle.protected(now) && !change.force {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "memory is protected; force or unprotect is required",
                ));
            }
            memory.deleted = true;
        }
        MemoryAction::Restore => {
            memory.deleted = false;
            memory.lifecycle.activation = MemoryActivation::Active;
            memory.lifecycle.inactive_since_unix_ms = None;
            memory.lifecycle.strength = memory.lifecycle.strength.max(REVIVE_THRESHOLD);
            memory.lifecycle.anchor_unix_ms = now.max(memory.lifecycle.anchor_unix_ms);
        }
        MemoryAction::Revise => {
            let edit = change
                .edit
                .as_ref()
                .ok_or_else(|| Error::invalid("revise requires edit"))?;
            if edit.title.is_none() && edit.content.is_none() && edit.epistemic_status.is_none() {
                return Err(Error::invalid("edit has no fields"));
            }
            if let Some(title) = &edit.title {
                validate_name(title, "title")?;
                memory.title = title.clone();
            }
            if let Some(content) = &edit.content {
                if content.trim().is_empty() || content.len() > 1024 * 1024 {
                    return Err(Error::invalid(
                        "memory content must contain 1 byte to 1 MiB",
                    ));
                }
                memory.content = content.clone();
            }
            if let Some(status) = &edit.epistemic_status {
                if !matches!(status.as_str(), "reported" | "inferred" | "uncertain") {
                    return Err(Error::invalid("unsupported epistemic status"));
                }
                memory.epistemic_status = status.clone();
            }
        }
        action => {
            memory.lifecycle = transition(
                &memory.lifecycle,
                action,
                now,
                change.strength.unwrap_or(1.0),
                change.hours,
            )?
        }
    }
    Ok(())
}
fn retention(state: &MemoryLifecycle, now: i64) -> f64 {
    state.strength.clamp(0.0, 1.0)
        * 2.0_f64.powf(
            -(now.saturating_sub(state.anchor_unix_ms).max(0) as f64) / (24.0 * HOUR_MS as f64),
        )
}
fn transition(
    state: &MemoryLifecycle,
    action: MemoryAction,
    now: i64,
    event_strength: f64,
    hours: Option<f64>,
) -> Result<MemoryLifecycle> {
    let now = now
        .max(state.anchor_unix_ms)
        .max(state.inactive_since_unix_ms.unwrap_or(i64::MIN))
        .max(state.last_reinforced_unix_ms.unwrap_or(i64::MIN));
    let mut result = state.clone();
    let current = retention(state, now);
    match action {
        MemoryAction::Protect => {
            if hours.is_none_or(|hours| hours <= 0.0) {
                result.pinned = true;
                result.protected_until_unix_ms = None;
            } else {
                result.pinned = false;
                result.protected_until_unix_ms =
                    Some(now.saturating_add((hours.unwrap() * HOUR_MS as f64) as i64));
            }
            return Ok(result);
        }
        MemoryAction::Unprotect => {
            result.pinned = false;
            result.protected_until_unix_ms = None;
            return Ok(result);
        }
        MemoryAction::Decay => {
            if state.activation == MemoryActivation::Active
                && !state.protected(now)
                && current <= FREEZE_THRESHOLD
            {
                result.activation = MemoryActivation::Decayed;
                result.inactive_since_unix_ms = Some(now);
            }
            return Ok(result);
        }
        MemoryAction::Freeze => {
            result.strength = current;
            result.activation = MemoryActivation::Frozen;
        }
        MemoryAction::Forget => {
            result.strength = 0.0;
            result.activation = MemoryActivation::Forgotten;
            result.pinned = false;
            result.protected_until_unix_ms = None;
        }
        MemoryAction::Reinforce => {
            result.strength =
                (current + (0.5 * event_strength).min(1.0) * (1.0 - current)).clamp(0.0, 1.0);
            result.last_reinforced_unix_ms = Some(now);
            let until =
                now.saturating_add(((24.0 * event_strength).max(1.0) * HOUR_MS as f64) as i64);
            result.protected_until_unix_ms = Some(
                result
                    .protected_until_unix_ms
                    .unwrap_or(i64::MIN)
                    .max(until),
            );
            if result.strength >= REVIVE_THRESHOLD {
                result.activation = MemoryActivation::Active;
            }
        }
        MemoryAction::Weaken => {
            result.strength = (current * (1.0 - (0.5 * event_strength).min(1.0))).clamp(0.0, 1.0);
            if result.strength <= FREEZE_THRESHOLD {
                result.activation = MemoryActivation::Weakened;
            } else if result.strength >= REVIVE_THRESHOLD {
                result.activation = MemoryActivation::Active;
            }
        }
        _ => return Err(Error::invalid("action is not a lifecycle transition")),
    }
    result.anchor_unix_ms = now;
    result.inactive_since_unix_ms = if result.activation == MemoryActivation::Active {
        None
    } else {
        state.inactive_since_unix_ms.or(Some(now))
    };
    Ok(result)
}
fn audit(
    ctx: &CallContext,
    memory: &Memory,
    action: &str,
    reason: &str,
    previous_revision: i64,
    details: Value,
    time: &OperationTime,
) -> MemoryAudit {
    MemoryAudit {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: memory.namespace_id.clone(),
        subject_id: memory.subject_id.clone(),
        memory_id: memory.id.clone(),
        action: action.into(),
        actor: ctx.caller_id.clone(),
        reason: reason.into(),
        previous_revision,
        revision: memory.revision,
        recorded_at: time.recorded(),
        commit_seq: memory.commit_seq,
        details,
    }
}
fn action_name(action: MemoryAction) -> String {
    serde_json::to_value(action)
        .expect("action is serializable")
        .as_str()
        .expect("action is a string")
        .to_owned()
}
