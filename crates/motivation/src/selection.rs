// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use chrono::{Datelike, Timelike};
/// The same read view authorizes metadata before loading any cognitive prose.
pub struct MotivationSelectionRequest<'a> {
    pub scope: &'a MotivationScope,
    pub source_read: &'a SourceReadScope,
    pub max_items: usize,
    pub max_bytes: usize,
    pub after: Option<Uuid>,
    pub at: Option<DateTime<Utc>>,
    pub as_of: Option<DateTime<Utc>>,
    pub history: bool,
}
pub struct MotivationSelection {
    pub records: Vec<MotivationRecord>,
    pub next: Option<Uuid>,
    pub omitted: bool,
}
#[derive(Debug)]
pub struct MotivationSourceEdges {
    pub reference: CognitiveRef,
    pub sources: Vec<CognitiveRef>,
    pub available: bool,
    pub read_bytes: usize,
}
impl MotivationService {
    /// Metadata-only exact edges. No prose/evidence materialization before authorization.
    pub async fn source_edges_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        ids: &[Uuid],
    ) -> Result<Vec<MotivationSourceEdges>> {
        if ids.len() > 2048 {
            return Err(Error::Invalid(
                "Motivation source metadata budget exceeded".into(),
            ));
        }
        let rows=sqlx::query("SELECT r.revision_id,o.family,o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' available,2*octet_length(r.content::text)+octet_length(r.reason)+2048+COALESCE((SELECT sum(2*octet_length(e.basis::text)+octet_length(e.role)+128) FROM motivation_evidence e WHERE e.revision_id=r.revision_id),0)::bigint read_bytes,COALESCE((SELECT jsonb_agg(jsonb_build_object('kind',d.ref_kind,'value',d.ref_value)) FROM motivation_dependencies d WHERE d.revision_id=r.revision_id),'[]'::jsonb) sources FROM motivation_revisions r JOIN motivation_objects o USING(object_id) WHERE o.subject_id=$1 AND r.revision_id=ANY($2::uuid[])").bind(subject.0).bind(ids).fetch_all(&mut **tx).await.map_err(db)?;
        rows.into_iter()
            .map(|r| {
                let family: String = r.try_get("family").map_err(db)?;
                let id: Uuid = r.try_get("revision_id").map_err(db)?;
                let sources: serde_json::Value = r.try_get("sources").map_err(db)?;
                Ok(MotivationSourceEdges {
                    reference: parse_reference(&format!("{family}_revision"), &id.to_string())?,
                    sources: sources
                        .as_array()
                        .expect("SQL array")
                        .iter()
                        .map(|s| {
                            parse_reference(
                                s["kind"].as_str().unwrap_or(""),
                                s["value"].as_str().unwrap_or(""),
                            )
                        })
                        .collect::<Result<Vec<_>>>()?,
                    available: r.try_get("available").map_err(db)?,
                    read_bytes: r.try_get::<i64, _>("read_bytes").map_err(db)? as usize,
                })
            })
            .collect()
    }
    pub async fn select(
        &self,
        subject: SubjectId,
        input: MotivationSelectionRequest<'_>,
    ) -> Result<MotivationSelection> {
        self.require_enabled(subject).await?;
        if input.max_items == 0
            || input.max_items > 2048
            || input.max_bytes == 0
            || input.max_bytes > 4 * 1024 * 1024
            || input.source_read.subject_id != Some(subject)
        {
            return Err(Error::Invalid(
                "invalid Motivation selection authorization/budget".into(),
            ));
        }
        let clock = self.runtime.read_clock(subject).await?;
        let at = input.at.unwrap_or(clock.at);
        let domain = if input.at.is_some() {
            "time:real"
        } else {
            &clock.time_domain_ref
        };
        let mut tx = self.store.read_view().await?;
        let read = self
            .roots
            .read_scope_in(&mut tx, subject, input.source_read)
            .await?;
        let entities = input
            .scope
            .entities
            .iter()
            .map(EntityRef::as_str)
            .collect::<Vec<_>>();
        let rows=sqlx::query(r#"SELECT r.revision_id,2*octet_length(r.content::text)+octet_length(r.reason)+2048+COALESCE((SELECT sum(2*octet_length(e.basis::text)+octet_length(e.role)+128) FROM motivation_evidence e WHERE e.revision_id=r.revision_id),0)::bigint read_bytes FROM motivation_objects o JOIN motivation_revisions r USING(object_id)  CROSS JOIN LATERAL (SELECT regexp_match(r.content#>>'{value,valid_time,at}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS instant_time,
 regexp_match(r.content#>>'{value,valid_time,start}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS start_time,
 regexp_match(r.content#>>'{value,valid_time,end}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS end_time) temporal WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND ($2::uuid IS NULL OR r.revision_id>$2) AND (($3::timestamptz IS NULL AND ($4 OR r.revision_id=o.current_revision_id)) OR (r.recorded_at<=$3 AND ($4 OR r.revision_id=(SELECT p.revision_id FROM motivation_revisions p WHERE p.object_id=o.object_id AND p.recorded_at<=$3 ORDER BY p.recorded_at DESC,p.revision_id DESC LIMIT 1)))) AND COALESCE(r.content#>'{value,scope,entities}','[]'::jsonb)<@to_jsonb($5::text[]) AND (r.content#>>'{value,scope,group}' IS NULL OR r.content#>>'{value,scope,group}'=$6) AND (r.content#>>'{value,scope,channel}' IS NULL OR r.content#>>'{value,scope,channel}'=$7) AND (r.content#>>'{value,scope,conversation}' IS NULL OR r.content#>>'{value,scope,conversation}'=$8) AND (r.content#>>'{value,scope,work_context}' IS NULL OR r.content#>>'{value,scope,work_context}'=$9) AND (r.content#>>'{value,scope,domain}' IS NULL OR r.content#>>'{value,scope,domain}'=$10) AND source_read_allowed($1,o.family||'_revision',r.revision_id::text,$12,$13,$14,$15) AND ((r.content#>>'{value,valid_time,kind}'='unknown' AND $20) OR (
 COALESCE(r.content#>>'{value,valid_time_domain_ref}','time:real')=$16 AND (
 (r.content#>>'{value,valid_time,kind}'='instant' AND ROW(temporal.instant_time[1]::integer,temporal.instant_time[2],rpad(COALESCE(temporal.instant_time[3],''),9,'0')::bigint)=ROW($17::integer,$18::text,$19::bigint))
 OR (r.content#>>'{value,valid_time,kind}'='interval'
 AND (r.content#>>'{value,valid_time,start}' IS NULL OR ROW(temporal.start_time[1]::integer,temporal.start_time[2],rpad(COALESCE(temporal.start_time[3],''),9,'0')::bigint)<=ROW($17::integer,$18::text,$19::bigint))
 AND (r.content#>>'{value,valid_time,end}' IS NULL OR ROW(temporal.end_time[1]::integer,temporal.end_time[2],rpad(COALESCE(temporal.end_time[3],''),9,'0')::bigint)>ROW($17::integer,$18::text,$19::bigint)))) )) ORDER BY r.revision_id LIMIT $11"#)
   .bind(subject.0).bind(input.after).bind(input.as_of).bind(input.history).bind(entities).bind(input.scope.group.as_ref().map(EntityRef::as_str)).bind(&input.scope.channel).bind(&input.scope.conversation).bind(input.scope.work_context.map(|w|w.0.to_string())).bind(&input.scope.domain).bind((input.max_items+1) as i64).bind(&read.root_keys).bind(&read.work_contexts).bind(&read.origin_conversation_refs).bind(serde_json::json!(&read.explicit_refs)).bind(domain).bind(at.year()).bind(at.format("%m-%dT%H:%M:%S").to_string()).bind(i64::from(at.nanosecond())).bind(input.at.is_none()).fetch_all(&mut *tx).await.map_err(db)?;
        let mut ids = Vec::new();
        let mut bytes = 0;
        let mut omitted = false;
        let mut last = None;
        for row in rows {
            let id: Uuid = row.try_get("revision_id").map_err(db)?;
            let cost = row.try_get::<i64, _>("read_bytes").map_err(db)? as usize;
            if ids.len() >= input.max_items || bytes + cost > input.max_bytes {
                omitted = true;
                break;
            }
            bytes += cost;
            ids.push(id);
            last = Some(id)
        }
        if omitted && ids.is_empty() {
            return Err(Error::Unavailable(
                "Motivation semantic unit exceeds read budget".into(),
            ));
        }
        let records = self.read_many_in(&mut tx, subject, &ids, true).await?;
        tx.commit().await.map_err(db)?;
        Ok(MotivationSelection {
            records,
            next: if omitted { last } else { None },
            omitted,
        })
    }
}
