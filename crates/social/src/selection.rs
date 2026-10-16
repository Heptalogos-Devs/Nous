// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use chrono::{Datelike, Timelike};

/// Authorization inputs are supplied by the trusted Host boundary. Scope is not authorization.
pub struct SocialSelectionRequest<'a> {
    pub scope: &'a SocialScope,
    pub source_read: &'a SourceReadScope,
    pub max_items: usize,
    pub max_bytes: usize,
    pub after: Option<Uuid>,
    pub expression: Option<&'a str>,
    pub endpoint: Option<&'a EntityRef>,
    pub counterpart: Option<&'a EntityRef>,
    pub at: Option<DateTime<Utc>>,
}
pub struct SocialSelection {
    pub records: Vec<SocialRecord>,
    pub next: Option<Uuid>,
    pub omitted: bool,
}
impl SocialService {
    /// Scope and source authorization are applied before body/evidence reads or ranking.
    pub async fn select(
        &self,
        subject: SubjectId,
        input: SocialSelectionRequest<'_>,
    ) -> Result<SocialSelection> {
        self.require_enabled(subject).await?;
        if input.max_items == 0
            || input.max_items > 2048
            || input.max_bytes == 0
            || input.max_bytes > 4 * 1024 * 1024
            || input.source_read.source_refs.len() > 1024
            || input.source_read.subject_id != Some(subject)
        {
            return Err(Error::Invalid("invalid Social selection budget".into()));
        }
        let mut tx = self.store.read_view().await?;
        let allowed = self
            .roots
            .roots_in(&mut tx, subject, &input.source_read.source_refs)
            .await?
            .into_iter()
            .map(|r| r.root_key)
            .collect::<Vec<_>>();
        let entities = input
            .scope
            .entities
            .iter()
            .map(|e| e.as_str())
            .collect::<Vec<_>>();
        let now = input.at.unwrap_or_else(|| self.runtime.now(subject));
        let rows=sqlx::query(r#"
SELECT o.object_id,r.revision_id,2*octet_length(r.content::text)+octet_length(r.reason)+1024+
 COALESCE((SELECT sum(2*octet_length(e.basis::text)+octet_length(e.role)+128) FROM social_evidence e WHERE e.revision_id=r.revision_id),0)::bigint AS read_bytes
FROM social_objects o JOIN social_revisions r ON r.revision_id=o.current_revision_id
 CROSS JOIN LATERAL (SELECT regexp_match(r.content#>>'{value,valid_time,at}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS instant_time,
 regexp_match(r.content#>>'{value,valid_time,start}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS start_time,
 regexp_match(r.content#>>'{value,valid_time,end}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS end_time) temporal

WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal'
 AND ($2::uuid IS NULL OR o.object_id>$2)
 AND (r.content#>>'{value,valid_time,kind}'='unknown'
 OR (r.content#>>'{value,valid_time,kind}'='instant' AND ROW(temporal.instant_time[1]::integer,temporal.instant_time[2],rpad(COALESCE(temporal.instant_time[3],''),9,'0')::bigint)=ROW($14::integer,$15::text,$16::bigint))
 OR (r.content#>>'{value,valid_time,kind}'='interval'
 AND (r.content#>>'{value,valid_time,start}' IS NULL OR ROW(temporal.start_time[1]::integer,temporal.start_time[2],rpad(COALESCE(temporal.start_time[3],''),9,'0')::bigint)<=ROW($14::integer,$15::text,$16::bigint))
 AND (r.content#>>'{value,valid_time,end}' IS NULL OR ROW(temporal.end_time[1]::integer,temporal.end_time[2],rpad(COALESCE(temporal.end_time[3],''),9,'0')::bigint)>ROW($14::integer,$15::text,$16::bigint))))
 AND COALESCE(r.content#>'{value,scope,entities}','[]'::jsonb)<@to_jsonb($3::text[])
 AND (r.content#>>'{value,scope,group}' IS NULL OR r.content#>>'{value,scope,group}'=$4)
 AND (r.content#>>'{value,scope,channel}' IS NULL OR r.content#>>'{value,scope,channel}'=$5)
 AND (r.content#>>'{value,scope,conversation}' IS NULL OR r.content#>>'{value,scope,conversation}'=$6)
 AND (r.content#>>'{value,scope,work_context}' IS NULL OR r.content#>>'{value,scope,work_context}'=$7)
 AND (r.content#>>'{value,scope,domain}' IS NULL OR r.content#>>'{value,scope,domain}'=$8)
 AND EXISTS(SELECT 1 FROM social_source_roots roots WHERE roots.revision_id=r.revision_id)
 AND NOT EXISTS(SELECT 1 FROM social_source_roots roots WHERE roots.revision_id=r.revision_id AND NOT(roots.root_key=ANY($9::text[])))
 AND ($10::text IS NULL OR (o.family='language_convention' AND (
  position(r.content#>>'{value,expression}' in $10)>0 OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(r.content#>'{value,variants}') v WHERE position(v in $10)>0)
  OR (COALESCE((r.content#>>'{value,case_insensitive}')::boolean,false) AND (position(lower(r.content#>>'{value,expression}') in lower($10))>0 OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(r.content#>'{value,variants}') v WHERE position(lower(v) in lower($10))>0))))))
 AND ($11::text IS NULL OR (o.family='social_relation' AND (r.content#>>'{value,from}'=$11 OR r.content#>>'{value,to}'=$11)))
 AND ($13::text IS NULL OR (r.content#>>'{value,from}'=$13 OR r.content#>>'{value,to}'=$13))
ORDER BY o.object_id LIMIT $12
"#).bind(subject.0).bind(input.after).bind(entities).bind(input.scope.group.as_ref().map(EntityRef::as_str)).bind(&input.scope.channel).bind(&input.scope.conversation).bind(input.scope.work_context.map(|v|v.0.to_string())).bind(&input.scope.domain).bind(allowed).bind(input.expression).bind(input.endpoint.map(EntityRef::as_str)).bind((input.max_items+1) as i64).bind(input.counterpart.map(EntityRef::as_str)).bind(now.year()).bind(now.format("%m-%dT%H:%M:%S").to_string()).bind(i64::from(now.nanosecond())).fetch_all(&mut *tx).await.map_err(db)?;
        let mut ids = Vec::new();
        let mut bytes = 0usize;
        let mut omitted = false;
        let mut last = None;
        for row in rows {
            let cost: i64 = row.try_get("read_bytes").map_err(db)?;
            if ids.len() >= input.max_items || bytes.saturating_add(cost as usize) > input.max_bytes
            {
                omitted = true;
                break;
            }
            bytes += cost as usize;
            ids.push(row.try_get::<Uuid, _>("revision_id").map_err(db)?);
            last = Some(row.try_get::<Uuid, _>("object_id").map_err(db)?);
        }
        if omitted && ids.is_empty() {
            return Err(Error::Unavailable(
                "Social semantic unit exceeds read budget".into(),
            ));
        }
        let records = self.read_many_in(&mut tx, subject, &ids, true).await?;
        tx.commit().await.map_err(db)?;
        Ok(SocialSelection {
            records,
            next: if omitted { last } else { None },
            omitted,
        })
    }
}
