// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

mod batch_read;
mod context;
mod lifecycle;
mod mutation;
mod provenance;
mod read;

pub(super) fn decode_memory_row(
    row: &sqlx::postgres::PgRow,
    subject: SubjectId,
) -> Result<(MemoryObject, MemoryRevision)> {
    let memory_id = MemoryId(row.try_get("memory_id").map_err(db)?);
    let revision_id = MemoryRevisionId(row.try_get("memory_revision_id").map_err(db)?);
    let object = MemoryObject {
        memory_id,
        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
        cognitive_role: parse_enum(
            ErrorOwner::Memory,
            row.try_get("cognitive_role").map_err(db)?,
            "cognitive role",
        )?,
        current_revision_id: MemoryRevisionId(row.try_get("current_revision_id").map_err(db)?),
        object_epoch: row.try_get("object_epoch").map_err(db)?,
        acceptance_state: parse_enum(
            ErrorOwner::Memory,
            row.try_get("acceptance_state").map_err(db)?,
            "acceptance state",
        )?,
        integrity_state: parse_enum(
            ErrorOwner::Memory,
            row.try_get("integrity_state").map_err(db)?,
            "integrity state",
        )?,
        suppression_state: parse_enum(
            ErrorOwner::Memory,
            row.try_get("suppression_state").map_err(db)?,
            "suppression state",
        )?,
        purge_state: parse_enum(
            ErrorOwner::Memory,
            row.try_get("purge_state").map_err(db)?,
            "purge state",
        )?,
        accessibility_mode: parse_enum(
            ErrorOwner::Memory,
            row.try_get("accessibility_mode").map_err(db)?,
            "accessibility mode",
        )?,
        created_at: row.try_get("created_at").map_err(db)?,
    };
    let revision = MemoryRevision {
        producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
        memory_revision_id: revision_id,
        memory_id,
        subject_id: subject,
        revision_no: row.try_get("revision_no").map_err(db)?,
        parent_revision_id: row
            .try_get::<Option<Uuid>, _>("parent_revision_id")
            .map_err(db)?
            .map(MemoryRevisionId),
        revision_intent: row
            .try_get::<Option<String>, _>("revision_intent")
            .map_err(db)?
            .map(|value| parse_enum(ErrorOwner::Memory, value, "revision intent"))
            .transpose()?,
        formation_mode: parse_enum(
            ErrorOwner::Memory,
            row.try_get("formation_mode").map_err(db)?,
            "formation mode",
        )?,
        grounding_occurrence_id: row
            .try_get::<Option<Uuid>, _>("grounding_occurrence_id")
            .map_err(db)?
            .map(OccurrenceId),
        semantic_role: row.try_get("semantic_role").map_err(db)?,
        title: row.try_get("title").map_err(db)?,
        representation_text: row.try_get("representation_text").map_err(db)?,
        epistemic_class: parse_enum(
            ErrorOwner::Memory,
            row.try_get("epistemic_class").map_err(db)?,
            "epistemic class",
        )?,
        valid_time: temporal_from_columns(
            ErrorOwner::Memory,
            row.try_get("valid_time_kind").map_err(db)?,
            row.try_get("valid_time_start").map_err(db)?,
            row.try_get("valid_time_end").map_err(db)?,
        )?,
        formed_at: row.try_get("formed_at").map_err(db)?,
        recorded_at: row.try_get("recorded_at").map_err(db)?,
    };
    Ok((object, revision))
}
