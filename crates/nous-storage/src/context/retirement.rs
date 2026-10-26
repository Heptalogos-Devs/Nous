use super::*;
use crate::{WriteTransaction, write_error};
use std::collections::BTreeSet;

fn affected(segment: &ContextSegment, targets: &BTreeSet<RetrievalTarget>) -> bool {
    segment
        .sources
        .iter()
        .chain(&segment.dependencies)
        .any(|target| targets.contains(target))
}
fn redact(segments: &mut [ContextSegment], targets: &BTreeSet<RetrievalTarget>) -> bool {
    let mut changed = false;
    for segment in segments {
        if affected(segment, targets) {
            segment.text.clear();
            segment.provenance = serde_json::Value::Null;
            segment.epistemic = None;
            changed = true;
        }
    }
    changed
}
fn redact_projection(
    projection: &mut ContextProjection,
    targets: &BTreeSet<RetrievalTarget>,
) -> bool {
    let keys = projection
        .segments
        .iter()
        .filter(|segment| affected(segment, targets))
        .map(|segment| segment.key.clone())
        .collect::<BTreeSet<_>>();
    if keys.is_empty() {
        return false;
    }
    redact(&mut projection.segments, targets);
    let view = &mut projection.epistemic_view;
    for group in [
        &mut view.accepted_facts_and_reports,
        &mut view.experiences,
        &mut view.execution_assistance,
        &mut view.unknowns,
        &mut view.familiar_domains,
        &mut view.learning_opportunities,
    ] {
        group.retain(|key| !keys.contains(key));
    }
    view.coverage.push("source_permanently_purged".into());
    projection.work_context = None;
    projection
        .degradation
        .push("source_permanently_purged".into());
    true
}
impl WriteTransaction {
    /// Returns task identities to cancel through Tasks in the same owning operation.
    pub async fn purge_context_sources(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        targets: &[RetrievalTarget],
    ) -> Result<Vec<String>> {
        self.invalidate_context_history(ns, subject, targets)
            .await?;
        let targets = targets.iter().cloned().collect::<BTreeSet<_>>();
        let mut jobs = BTreeSet::new();
        let rows=sqlx::query("SELECT p.id,p.job_id,p.catalog,p.response,p.steward_job_id FROM context_preparations p JOIN task_records j ON j.job_id=p.job_id WHERE j.namespace_id=? AND j.subject_id=?")
            .bind(ns.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let mut catalog: Vec<ContextSegment> = decode(row.get("catalog"))?;
            if !redact(&mut catalog, &targets) {
                continue;
            }
            let job: String = row.get("job_id");
            jobs.insert(job);
            let child: Option<String> = row.get("steward_job_id");
            if let Some(child) = child {
                self.redact_job_calls(&child, "context source permanently purged")
                    .await?;
                jobs.insert(child);
            }
            let mut response: Option<ManagedContextResponse> = row
                .get::<Option<&str>, _>("response")
                .map(decode)
                .transpose()?;
            if let Some(response) = &mut response {
                redact_projection(&mut response.projection, &targets);
            }
            sqlx::query("UPDATE context_preparations SET catalog=?,response=?,work_context=NULL,degradation=json_insert(degradation,'$[#]','source_permanently_purged') WHERE id=?")
                .bind(encode(&catalog)?).bind(response.as_ref().map(encode).transpose()?).bind(row.get::<String,_>("id")).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        let rows = sqlx::query(
            "SELECT id,body FROM context_projections WHERE namespace_id=? AND subject_id=?",
        )
        .bind(ns.as_str())
        .bind(subject.as_str())
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)?;
        for row in rows {
            let mut projection: ContextProjection = decode(row.get("body"))?;
            if redact_projection(&mut projection, &targets) {
                sqlx::query("UPDATE context_projections SET body=? WHERE id=?")
                    .bind(encode(&projection)?)
                    .bind(row.get::<String, _>("id"))
                    .execute(&mut *self.tx)
                    .await
                    .map_err(write_error)?;
            }
        }
        Ok(jobs.into_iter().collect())
    }
}
