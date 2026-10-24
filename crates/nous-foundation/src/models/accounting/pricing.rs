use nous_domain::{
    CostMissing, DecimalAmount, ModelCost, ModelPriceSnapshot, ModelUsage, Money, UsageKind,
};
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

pub fn cost(
    kind: UsageKind,
    usage: Option<&ModelUsage>,
    pricing: Option<ModelPriceSnapshot>,
    body: Option<&Value>,
) -> ModelCost {
    let mut missing = Vec::new();
    let estimated = estimate(kind, usage, pricing.as_ref(), &mut missing);
    let evidence = billing_evidence(body);
    let reported = reported(evidence, &mut missing);
    ModelCost {
        pricing,
        estimated,
        reported,
        missing,
        reported_evidence: evidence
            .map(|(value, currency)| serde_json::json!({"value":value,"currency":currency})),
    }
}
fn estimate(
    kind: UsageKind,
    usage: Option<&ModelUsage>,
    snapshot: Option<&ModelPriceSnapshot>,
    missing: &mut Vec<CostMissing>,
) -> Option<Money> {
    let Some(snapshot) = snapshot else {
        missing.push(CostMissing::Pricing);
        return None;
    };
    let Some(input) = usage.and_then(|usage| usage.input_tokens) else {
        missing.push(CostMissing::InputTokens);
        return None;
    };
    let rate = &snapshot.pricing;
    let input = charge(input, rate.input_per_million.decimal(), missing)?;
    let output = usage.and_then(|usage| usage.output_tokens);
    if output.is_none()
        && (kind == UsageKind::Chat
            || usage.is_some_and(|usage| {
                usage
                    .invalid_fields
                    .contains(&nous_domain::UsageField::OutputTokens)
            }))
    {
        missing.push(CostMissing::OutputTokens);
        return None;
    }
    let amount = if let Some(output) = output {
        let output = charge(output, rate.output_per_million.decimal(), missing)?;
        let total = input.checked_add(output).or_else(|| {
            missing.push(CostMissing::ArithmeticOverflow);
            None
        })?;
        if total.checked_sub(input) != Some(output) || total.checked_sub(output) != Some(input) {
            missing.push(CostMissing::ArithmeticPrecision);
            return None;
        }
        total
    } else {
        input
    };
    let amount = DecimalAmount::new(amount).ok()?;
    Money::new(rate.currency.clone(), amount).ok()
}
fn charge(tokens: u64, price: Decimal, missing: &mut Vec<CostMissing>) -> Option<Decimal> {
    let numerator = Decimal::from(tokens).checked_mul(price).or_else(|| {
        missing.push(CostMissing::ArithmeticOverflow);
        None
    })?;
    let million = Decimal::from(1_000_000);
    let value = numerator.checked_div(million).or_else(|| {
        missing.push(CostMissing::ArithmeticOverflow);
        None
    })?;
    if value.checked_mul(million) != Some(numerator)
        || (tokens > 0 && !price.is_zero() && value.is_zero())
    {
        missing.push(CostMissing::ArithmeticPrecision);
        return None;
    }
    Some(value)
}
fn billing_evidence(body: Option<&Value>) -> Option<(&Value, Option<&Value>)> {
    let body = body?;
    let value = body
        .pointer("/usage/cost")
        .filter(|value| !value.is_null())
        .or_else(|| body.get("cost").filter(|value| !value.is_null()))?;
    let currency = if value.is_object() {
        value.get("currency")
    } else {
        body.pointer("/usage/cost_currency")
            .filter(|value| !value.is_null())
            .or_else(|| body.pointer("/usage/currency"))
    };
    Some((value, currency))
}
fn reported(
    evidence: Option<(&Value, Option<&Value>)>,
    missing: &mut Vec<CostMissing>,
) -> Option<Money> {
    let (value, currency) = evidence?;
    let amount = if value.is_object() {
        value.get("amount")
    } else {
        Some(value)
    };
    let currency = currency.and_then(Value::as_str);
    let Some(currency) = currency else {
        missing.push(CostMissing::ProviderCurrency);
        return None;
    };
    let amount = amount.and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    });
    let money = amount
        .and_then(|amount| {
            DecimalAmount::from_str(&amount).ok().or_else(|| {
                Decimal::from_scientific(&amount)
                    .ok()
                    .and_then(|amount| DecimalAmount::new(amount).ok())
            })
        })
        .and_then(|amount| Money::new(currency.to_ascii_uppercase(), amount).ok());
    if money.is_none() {
        missing.push(CostMissing::InvalidProviderCost);
    }
    money
}
