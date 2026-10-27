use nous_domain::*;
use std::collections::BTreeMap;
type ProducerIdentity = (NamespaceId, SubjectId, ModelCallId);
type RecordedAccounting = (Option<ModelUsage>, Option<ModelCost>);

pub(super) struct Accounting {
    pub totals: SubjectModelDiagnostics,
    imported: BTreeMap<ProducerIdentity, RecordedAccounting>,
}
impl Accounting {
    pub fn new() -> Self {
        Self {
            totals: Default::default(),
            imported: BTreeMap::new(),
        }
    }
    pub fn record(&mut self, call: &ModelCall) -> Result<()> {
        let target = match &call.origin {
            ModelCallOrigin::Local => &mut self.totals.local,
            ModelCallOrigin::Imported {
                original_call_id,
                original_namespace_id,
                original_subject_id,
                ..
            } => {
                self.totals.imported_records += 1;
                let identity = (
                    original_namespace_id.clone(),
                    original_subject_id.clone(),
                    original_call_id.clone(),
                );
                let evidence = (call.usage.clone(), call.cost.clone());
                if let Some(previous) = self.imported.get(&identity) {
                    if serde_json::to_value(previous)? != serde_json::to_value(&evidence)? {
                        return Err(Error::new(
                            ErrorCode::Conflict,
                            "imported producer copies disagree about their accounting",
                        )
                        .with_context("original_call_id", serde_json::json!(original_call_id)));
                    }
                    self.totals.imported_duplicate_records += 1;
                    return Ok(());
                }
                self.imported.insert(identity, evidence);
                &mut self.totals.imported_history
            }
        };
        target.calls += 1;
        *target.by_state.entry(name(&call.state)?).or_default() += 1;
        let kind = call
            .usage
            .as_ref()
            .map(|usage| name(&usage.kind))
            .transpose()?
            .or_else(|| {
                call.request
                    .pointer("/preview/input/kind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unknown".into());
        *target.by_kind.entry(kind).or_default() += 1;
        tokens(
            &mut target.input_tokens,
            call.usage.as_ref().and_then(|usage| usage.input_tokens),
        );
        tokens(
            &mut target.output_tokens,
            call.usage.as_ref().and_then(|usage| usage.output_tokens),
        );
        tokens(
            &mut target.total_tokens,
            call.usage.as_ref().and_then(ModelUsage::known_total_tokens),
        );
        money(
            &mut target.estimated_cost,
            call.cost.as_ref().and_then(|cost| cost.estimated.as_ref()),
        )?;
        money(
            &mut target.reported_cost,
            call.cost.as_ref().and_then(|cost| cost.reported.as_ref()),
        )?;
        Ok(())
    }
    pub fn finish(mut self) -> SubjectModelDiagnostics {
        for group in [&mut self.totals.local, &mut self.totals.imported_history] {
            for total in [
                &mut group.input_tokens,
                &mut group.output_tokens,
                &mut group.total_tokens,
            ] {
                if total.contributing_calls == 0 && total.missing_calls > 0 {
                    total.known = None;
                }
            }
        }
        self.totals
    }
}
fn tokens(total: &mut TokenTotal, value: Option<u64>) {
    let Some(value) = value else {
        total.missing_calls += 1;
        return;
    };
    total.contributing_calls += 1;
    if !total.overflow {
        total.known = total.known.and_then(|known| known.checked_add(value));
        total.overflow = total.known.is_none();
    }
}
fn money(total: &mut CostTotals, value: Option<&Money>) -> Result<()> {
    let Some(value) = value else {
        total.missing_calls += 1;
        return Ok(());
    };
    if total.arithmetic_issues.contains_key(&value.currency) {
        return Ok(());
    }
    let (amount, issue) = if let Some(current) = total.by_currency.get(&value.currency) {
        match current.decimal().checked_add(value.amount.decimal()) {
            None => (None, "overflow"),
            Some(sum)
                if sum.checked_sub(current.decimal()) != Some(value.amount.decimal())
                    || sum.checked_sub(value.amount.decimal()) != Some(current.decimal()) =>
            {
                (None, "precision")
            }
            Some(sum) => (Some(sum), ""),
        }
    } else {
        (Some(value.amount.decimal()), "")
    };
    if let Some(amount) = amount {
        total
            .by_currency
            .insert(value.currency.clone(), DecimalAmount::new(amount)?);
    } else {
        total.by_currency.remove(&value.currency);
        total
            .arithmetic_issues
            .insert(value.currency.clone(), issue.into());
    }
    Ok(())
}
pub(super) fn name(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::internal("diagnostic key is not an enum name"))
}
