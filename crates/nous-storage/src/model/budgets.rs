//! One Authority transaction admits every applicable budget and the external attempt together.
use crate::{WriteTransaction, storage_error, write_error};
use chrono::{DateTime, Utc};
use nous_domain::*;
use sqlx::Row;
use std::collections::BTreeMap;

pub enum ModelBudgetAdmission {
    Reserved,
    AlreadyReserved,
    Exhausted(Vec<ModelBudgetUsage>),
}

impl WriteTransaction {
    /// Failed and uncertain outcomes keep these immutable charges. Source retirement only redacts content.
    pub async fn reserve_model_call(
        &mut self,
        call: &ModelCall,
        plan: &ModelBudgetPlan,
    ) -> Result<ModelBudgetAdmission> {
        if call.origin != ModelCallOrigin::Local {
            return Err(Error::invalid(
                "imported execution history cannot reserve local budgets",
            ));
        }
        call.id.validate()?;
        let limits = &plan.limits;
        if plan.context.model_id != call.resource_id
            || plan.context.scope
                != (ConfigScope::Subject {
                    namespace_id: call.namespace_id.clone(),
                    subject_id: call.subject_id.clone(),
                })
            || limits
                .iter()
                .any(|limit| !plan.context.permits(&limit.bucket))
        {
            return Err(Error::invalid(
                "budget buckets differ from the invocation's resource, role or scope",
            ));
        }
        if call.state != ModelCallState::Started
            || call.response.is_some()
            || call.completed_at.is_some()
        {
            return Err(Error::invalid(
                "only a new external attempt can reserve model budgets",
            ));
        }
        if limits.len() < 3 || limits.len() > 5 {
            return Err(Error::invalid(
                "model attempt requires two public resource buckets and 1..3 role scopes",
            ));
        }
        let providers = limits
            .iter()
            .filter(|limit| matches!(limit.bucket, ModelBudgetBucket::Provider { .. }))
            .count();
        let models=limits.iter().filter(|limit|matches!(&limit.bucket,ModelBudgetBucket::Model{id} if id==&call.resource_id)).count();
        if providers != 1 || models != 1 {
            return Err(Error::invalid(
                "model attempt requires its provider and model budget",
            ));
        }
        let day = DateTime::parse_from_rfc3339(&call.created_at)
            .map_err(Error::dependency)?
            .with_timezone(&Utc)
            .date_naive()
            .to_string();
        let mut buckets = BTreeMap::new();
        for limit in limits {
            validate_daily_call_limit(limit.max_calls_per_day)?;
            let key = limit.bucket.key()?;
            if buckets.insert(key, limit).is_some() {
                return Err(Error::invalid("model attempt repeats a budget bucket"));
            }
        }
        let existing = sqlx::query("SELECT request,job_id,step_key,attempt,resource_id,namespace_id,subject_id,trace_id,created_at FROM model_calls WHERE id=?")
            .bind(call.id.as_str()).fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
        if let Some(existing) = existing {
            let request: serde_json::Value = serde_json::from_str(existing.get("request"))?;
            let same = request == call.request
                && existing.get::<String, _>("job_id") == call.job_id
                && existing.get::<String, _>("step_key") == call.step_key
                && existing.get::<i64, _>("attempt") == call.attempt
                && existing.get::<String, _>("resource_id") == call.resource_id
                && existing.get::<String, _>("namespace_id") == call.namespace_id.as_str()
                && existing.get::<String, _>("subject_id") == call.subject_id.as_str()
                && existing.get::<String, _>("trace_id") == call.trace_id.as_str()
                && existing.get::<String, _>("created_at") == call.created_at;
            let charged: Vec<String> = sqlx::query_scalar(
                "SELECT bucket FROM model_budget_charges WHERE call_id=? ORDER BY bucket",
            )
            .bind(call.id.as_str())
            .fetch_all(&mut *self.tx)
            .await
            .map_err(storage_error)?;
            if !same || charged != buckets.keys().cloned().collect::<Vec<_>>() {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "model call ID already owns a different reservation",
                ));
            }
            return Ok(ModelBudgetAdmission::AlreadyReserved);
        }
        let exhausted = exhausted_in(&mut self.tx, plan, &day).await?;
        if !exhausted.is_empty() {
            return Ok(ModelBudgetAdmission::Exhausted(exhausted));
        }
        self.insert_model_call(call).await?;
        for (bucket, limit) in buckets {
            sqlx::query("INSERT INTO model_budget_charges(call_id,bucket,day,max_calls_per_day) VALUES(?,?,?,?)")
                .bind(call.id.as_str()).bind(bucket).bind(&day).bind(i64::from(limit.max_calls_per_day))
                .execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(ModelBudgetAdmission::Reserved)
    }
}

impl crate::ReadView {
    pub async fn exhausted_model_budgets(
        &mut self,
        plan: &ModelBudgetPlan,
        day: &str,
    ) -> Result<Vec<ModelBudgetUsage>> {
        exhausted_in(&mut self.tx, plan, day).await
    }
}

async fn exhausted_in(
    conn: &mut sqlx::SqliteConnection,
    plan: &ModelBudgetPlan,
    day: &str,
) -> Result<Vec<ModelBudgetUsage>> {
    let mut exhausted = Vec::new();
    for limit in &plan.limits {
        let key = limit.bucket.key()?;
        let used: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM model_budget_charges WHERE bucket=? AND day=?",
        )
        .bind(key)
        .bind(day)
        .fetch_one(&mut *conn)
        .await
        .map_err(storage_error)?;
        if used >= i64::from(limit.max_calls_per_day) {
            exhausted.push(ModelBudgetUsage {
                limit: limit.clone(),
                used,
            });
        }
    }
    Ok(exhausted)
}
