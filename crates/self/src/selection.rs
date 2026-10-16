// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use chrono::{Datelike, Timelike};

impl SelfService {
    /// Filter metadata before reading bounded complete semantic units. Never scan bodies into RAM.
    pub async fn select(
        &self,
        subject: SubjectId,
        input: SelfSelectionRequest<'_>,
    ) -> Result<SelfSelection> {
        self.require_enabled(subject).await?;
        if input.max_items == 0
            || input.max_items > if input.automatic { 2048 } else { 128 }
            || input.max_bytes > 4 * 1024 * 1024
            || input.targets.len() > 128
            || input.sources.len() > 128
            || input.ranked.len() > 256
        {
            return Err(Error::Invalid("invalid Self selection budget".into()));
        }
        if input
            .targets
            .iter()
            .chain(input.ranked)
            .any(|r| r.domain() != CognitiveDomain::SelfCognition)
        {
            return Err(Error::Invalid(
                "Self selection requires owned targets/ranked references".into(),
            ));
        }
        let clock = self.runtime.read_clock(subject).await?;
        let mut tx = self.store.read_view().await?;
        let mut result = SelfSelection {
            records: Vec::new(),
            degradation: Vec::new(),
        };
        let now = clock.at;
        let mut remaining = input.max_bytes;
        let mut omitted = false;
        // Identity is read separately. Optional/frequently-created records cannot displace it.
        for required in [true, false] {
            let limit = input.max_items.saturating_sub(result.records.len());
            let rows = self
                .selection_rows(
                    &mut tx,
                    subject,
                    &input,
                    required,
                    limit + 1,
                    now,
                    &clock.time_domain_ref,
                )
                .await?;
            if required && rows.len() > limit {
                return Err(Error::Unavailable(
                    "required Self identity exceeds candidate budget".into(),
                ));
            }
            let mut ids = Vec::new();
            for row in rows {
                let revision: Uuid = row.try_get("revision_id").map_err(db)?;
                let cost: i64 = row.try_get("read_bytes").map_err(db)?;
                let targeted: bool = row.try_get("targeted").map_err(db)?;
                let exceeds = ids.len() >= limit || cost as usize > remaining;
                if exceeds && (required || targeted) {
                    return Err(Error::Unavailable(
                        "required Self identity/explicit target exceeds read budget".into(),
                    ));
                }
                if exceeds {
                    omitted = true;
                    continue;
                }
                ids.push(revision);
                remaining = remaining.saturating_sub(cost as usize);
            }
            result
                .records
                .extend(self.read_many_in(&mut tx, subject, &ids, true).await?);
        }
        for target in input.targets {
            if !result
                .records
                .iter()
                .any(|r| r.reference() == *target || r.object_reference() == *target)
            {
                return Err(Error::Unavailable(format!(
                    "explicit Self target is outside available current selection: {target}"
                )));
            }
        }
        if omitted {
            result.degradation.push(Degradation { code: "self_candidate_omission".into(), detail: Some(format!("Complete optional records omitted at {} candidates / {} body-and-support bytes; unselected Self may exist", input.max_items, input.max_bytes)) });
        }
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
    async fn selection_rows(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        input: &SelfSelectionRequest<'_>,
        required: bool,
        limit: usize,
        now: DateTime<Utc>,
        domain: &str,
    ) -> Result<Vec<sqlx::postgres::PgRow>> {
        let refs =
            |values: &[CognitiveRef]| values.iter().map(ToString::to_string).collect::<Vec<_>>();
        let entities = input
            .scope
            .entities
            .iter()
            .map(|e| e.as_str().to_owned())
            .collect::<Vec<_>>();
        let kinds = input
            .sources
            .iter()
            .map(|r| reference_parts(r).0)
            .collect::<Vec<_>>();
        let values = input
            .sources
            .iter()
            .map(|r| reference_parts(r).1)
            .collect::<Vec<_>>();
        // Owner-serialized UTC tuples preserve nanosecond and extended-year comparisons;
        // timestamptz casts would round valid_time before candidate budgets.
        // Natural-language scope is retained for interpretation, never used as a SQL predicate.
        sqlx::query(r#"
WITH anchors AS (
 SELECT kind,value FROM unnest($5::text[],$6::text[]) s(kind,value)
 UNION
 SELECT o.family||'_revision',o.current_revision_id::text FROM self_objects o
 WHERE o.subject_id=$1 AND (o.family||':'||o.object_id::text=ANY($3::text[]) OR o.family||'_revision:'||o.current_revision_id::text=ANY($3::text[]))
), eligible AS (
 SELECT o.family,o.object_id,o.current_revision_id AS revision_id,r.content,r.reason,r.recorded_at,
  o.family||':'||o.object_id::text=ANY($3::text[]) OR o.family||'_revision:'||r.revision_id::text=ANY($3::text[]) AS targeted,
  COALESCE(array_position($4::text[],o.family||'_revision:'||r.revision_id::text),array_position($4::text[],o.family||':'||o.object_id::text)) AS rank,
  (EXISTS(SELECT 1 FROM self_dependencies d JOIN anchors s ON d.ref_kind=s.kind AND d.ref_value=s.value WHERE d.revision_id=r.revision_id)
   OR EXISTS(SELECT 1 FROM self_dependencies d JOIN anchors a ON d.revision_id::text=a.value AND a.kind IN ('self_facet_revision','narrative_thread_revision') JOIN self_revisions dependency ON dependency.revision_id::text=d.ref_value AND dependency.object_id=o.object_id WHERE d.ref_kind=o.family||'_revision')) AS related,
  (o.family='self_facet' AND r.content#>>'{value,kind}'='identity' AND r.content#>>'{value,stance}'='adopted') AS identity
 FROM self_objects o JOIN self_revisions r ON r.revision_id=o.current_revision_id
 CROSS JOIN LATERAL (SELECT regexp_match(r.content#>>'{value,valid_time,at}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS instant_time,
 regexp_match(r.content#>>'{value,valid_time,start}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS start_time,
 regexp_match(r.content#>>'{value,valid_time,end}', '^([+-]?[0-9]+)-([0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\.([0-9]+))?Z$') AS end_time) temporal
 WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.suppression_state='normal'
 AND (NOT $2 OR (o.family='self_facet' AND o.integrity_state='valid'
  AND (r.content#>>'{value,valid_time,kind}'='unknown' OR COALESCE(r.content#>>'{value,valid_time_domain_ref}','time:real')=$20)
  AND (r.content#>>'{value,valid_time,kind}'='unknown'
   OR (r.content#>>'{value,valid_time,kind}'='instant' AND ROW(temporal.instant_time[1]::integer,temporal.instant_time[2],rpad(COALESCE(temporal.instant_time[3],''),9,'0')::bigint)=ROW($7::integer,$14::text,$15::bigint))
   OR (r.content#>>'{value,valid_time,kind}'='interval'
    AND (r.content#>>'{value,valid_time,start}' IS NULL OR ROW(temporal.start_time[1]::integer,temporal.start_time[2],rpad(COALESCE(temporal.start_time[3],''),9,'0')::bigint)<=ROW($7::integer,$14::text,$15::bigint))
    AND (r.content#>>'{value,valid_time,end}' IS NULL OR ROW(temporal.end_time[1]::integer,temporal.end_time[2],rpad(COALESCE(temporal.end_time[3],''),9,'0')::bigint)>ROW($7::integer,$14::text,$15::bigint))))))
 AND (NOT $2 OR o.family<>'self_facet' OR (
  (COALESCE(jsonb_array_length(r.content#>'{value,scope,purposes}'),0)=0 OR (NOT $2 AND cardinality($8::text[])=0) OR (r.content#>'{value,scope,purposes}') ?| $8::text[])
  AND (r.content#>>'{value,scope,work_context}' IS NULL OR (NOT $2 AND $9::text IS NULL) OR r.content#>>'{value,scope,work_context}'=$9)
  AND (r.content#>>'{value,scope,conversation}' IS NULL OR (NOT $2 AND $10::text IS NULL) OR r.content#>>'{value,scope,conversation}'=$10)
  AND ((NOT $2 AND cardinality($11::text[])=0) OR COALESCE(r.content#>'{value,scope,entities}','[]'::jsonb)<@to_jsonb($11::text[]))
 ))
)
SELECT revision_id,targeted,
 2*octet_length(content::text)+octet_length(reason)+1024+
 COALESCE((SELECT sum(2*octet_length(e.basis::text)+octet_length(e.role)+128) FROM self_evidence e WHERE e.revision_id=eligible.revision_id),0)::bigint AS read_bytes
FROM eligible
WHERE ($16::text[] IS NULL OR CASE WHEN $2 THEN source_read_allowed($1,family||'_revision',revision_id::text,$16,$17,$18,$19) ELSE source_read_metadata_allowed($1,family||'_revision',revision_id::text,$16,$17,$18,$19) END) AND (NOT $2 OR identity=$12) AND ($2 OR targeted OR related OR rank IS NOT NULL OR identity)
 AND ($2 OR NOT $12)
ORDER BY targeted DESC,related DESC,rank NULLS LAST,recorded_at DESC,revision_id
LIMIT $13
"#)
        .bind(subject.0).bind(input.automatic).bind(refs(input.targets)).bind(refs(input.ranked))
        .bind(kinds).bind(values).bind(now.year()).bind(&input.scope.purposes)
        .bind(input.scope.work_context.map(|w| w.0.to_string())).bind(&input.scope.conversation)
        .bind(entities).bind(required).bind(limit as i64).bind(now.format("%m-%dT%H:%M:%S").to_string()).bind(i64::from(now.nanosecond() % 1_000_000_000)).bind(input.source_read.map(|r|r.root_keys.clone())).bind(input.source_read.map(|r|r.work_contexts.clone()).unwrap_or_default()).bind(input.source_read.map(|r|r.origin_conversation_refs.clone()).unwrap_or_default()).bind(input.source_read.map(|r|serde_json::json!(&r.explicit_refs)).unwrap_or_else(||serde_json::json!([]))).bind(domain).fetch_all(&mut **tx).await.map_err(db)
    }
}

impl SelfService {
    /// Complete permitted metadata set for shared Serving filtering; never a ranked first page.
    pub async fn authorized_references(
        &self,
        subject: SubjectId,
        read: &SourceReadView,
        automatic: bool,
    ) -> Result<Vec<CognitiveRef>> {
        if read.subject != subject {
            return Err(Error::Invalid("Self source read Subject mismatch".into()));
        }
        let rows:Vec<(String,String)>=sqlx::query_as("SELECT o.family||'_revision',o.current_revision_id::text FROM self_objects o WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND (NOT $6 OR o.integrity_state='valid') AND o.suppression_state='normal' AND CASE WHEN $6 THEN source_read_allowed($1,o.family||'_revision',o.current_revision_id::text,$2,$3,$4,$5) ELSE source_read_metadata_allowed($1,o.family||'_revision',o.current_revision_id::text,$2,$3,$4,$5) END").bind(subject.0).bind(&read.root_keys).bind(&read.work_contexts).bind(&read.origin_conversation_refs).bind(serde_json::json!(&read.explicit_refs)).bind(automatic).fetch_all(self.store.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|(k, v)| parse_reference(&k, &v))
            .collect()
    }
}
