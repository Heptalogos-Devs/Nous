use super::Images;
use anyhow::Result;
use serde_json::{Map, Number, Value, json};
use std::collections::BTreeMap;
pub(super) fn bounded_json(bytes: &[u8], remaining: &mut usize, depth: usize) -> Result<Value> {
    Ok(nous_foundation::json::bounded_value(
        bytes, remaining, depth,
    )?)
}
/// Python json.dumps(sort_keys=True,separators=(',',':'),ensure_ascii=False) compatibility.
pub(crate) fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    let mut text = String::new();
    canonical_into(value, &mut text)?;
    Ok(text.into_bytes())
}
fn canonical_into(value: &Value, output: &mut String) -> Result<()> {
    match value {
        Value::Object(fields) => {
            output.push('{');
            let mut keys: Vec<_> = fields.keys().collect();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key)?);
                output.push(':');
                canonical_into(&fields[key], output)?;
            }
            output.push('}');
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                canonical_into(value, output)?;
            }
            output.push(']');
        }
        Value::Number(number) if number.is_f64() => output.push_str(&python_float(number)),
        _ => output.push_str(&serde_json::to_string(value)?),
    }
    Ok(())
}
fn python_float(number: &Number) -> String {
    let raw = number.to_string();
    let (sign, raw) = if let Some(raw) = raw.strip_prefix('-') {
        ("-", raw)
    } else {
        ("", raw.as_str())
    };
    let (mantissa, exponent) = raw
        .split_once(['e', 'E'])
        .map(|(mantissa, exponent)| {
            (
                mantissa,
                exponent.parse::<i32>().expect("JSON number exponent"),
            )
        })
        .unwrap_or((raw, 0));
    let mut point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let mut digits = mantissa.replace('.', "");
    let leading = digits.bytes().take_while(|byte| *byte == b'0').count();
    digits.drain(..leading);
    point -= leading as i32;
    while digits.ends_with('0') {
        digits.pop();
    }
    if digits.is_empty() {
        return format!("{sign}0.0");
    }
    let power = point - 1;
    if !(-4..16).contains(&power) {
        let mut mantissa = digits[..1].to_owned();
        if digits.len() > 1 {
            mantissa.push('.');
            mantissa.push_str(&digits[1..]);
        }
        return format!(
            "{sign}{mantissa}e{}{power_abs:02}",
            if power < 0 { '-' } else { '+' },
            power_abs = power.unsigned_abs()
        );
    }
    if point <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
    } else if point as usize >= digits.len() {
        format!(
            "{sign}{digits}{}.0",
            "0".repeat(point as usize - digits.len())
        )
    } else {
        format!(
            "{sign}{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    }
}

fn evidence(value: &Value, remaining: &mut usize, depth: usize) -> Result<Value> {
    let mut object = match value {
        Value::Object(value) => value.clone(),
        Value::String(text) if !text.trim().is_empty() => {
            match bounded_json(text.as_bytes(), remaining, depth) {
                Ok(Value::Object(object)) => object,
                Ok(_) => Map::new(),
                // The upstream digest treats legacy malformed evidence as {} while
                // retaining the original evidence_json in the portable record.
                Err(error)
                    if error
                        .downcast_ref::<serde_json::Error>()
                        .is_some_and(|error| error.is_syntax() || error.is_eof()) =>
                {
                    Map::new()
                }
                Err(error) => return Err(error),
            }
        }
        _ => Map::new(),
    };
    for key in [
        "chat_id",
        "external_ref",
        "imported",
        "message_id",
        "source_link_id",
    ] {
        object.remove(key);
    }
    Ok(Value::Object(object))
}
pub(crate) fn canonical_images(
    images: &Images,
    remaining: &mut usize,
    depth: usize,
) -> Result<Value> {
    let assets: BTreeMap<_, _> = images
        .assets
        .iter()
        .map(|asset| (asset.asset_id.as_str(), asset))
        .collect();
    let mut occurrences: Vec<_> = images.occurrences.iter().collect();
    occurrences.sort_by_key(|item| {
        (
            assets
                .get(item.asset_id.as_str())
                .map(|asset| asset.content_hash.as_str())
                .unwrap_or(""),
            item.component_path.as_str(),
            item.occurrence_id.as_str(),
        )
    });
    let keys: BTreeMap<_, _> = occurrences
        .iter()
        .enumerate()
        .map(|(index, item)| (item.occurrence_id.as_str(), format!("occurrence-{index}")))
        .collect();
    let mut asset_values:Vec<_>=images.assets.iter().map(|asset|json!({"byte_size":asset.byte_size,"content_hash":asset.content_hash,"height":asset.height,"mime_type":asset.mime_type,"width":asset.width})).collect();
    asset_values.sort_by(|a, b| a["content_hash"].as_str().cmp(&b["content_hash"].as_str()));
    let occurrence_values:Vec<_>=occurrences.iter().map(|item|json!({"asset_hash":assets.get(item.asset_id.as_str()).map(|asset|asset.content_hash.as_str()).unwrap_or(""),"component_path":item.component_path})).collect();
    let mut observations=images.observations.iter().map(|item|Ok(json!({"confirm_status":item.confirm_status,"occurrence":keys.get(item.occurrence_id.as_str()).map(String::as_str).unwrap_or(""),"source_kind":item.source_kind,"text":item.text,"evidence":evidence(&item.evidence_json,remaining,depth)?}))).collect::<Result<Vec<Value>>>()?;
    observations.sort_by(|a, b| {
        (
            a["occurrence"].as_str(),
            a["source_kind"].as_str(),
            a["text"].as_str(),
        )
            .cmp(&(
                b["occurrence"].as_str(),
                b["source_kind"].as_str(),
                b["text"].as_str(),
            ))
    });
    let mut links=images.links.iter().map(|item|Ok(json!({"link_kind":item.link_kind,"occurrence":keys.get(item.occurrence_id.as_str()).map(String::as_str).unwrap_or(""),"target_id":item.target_id,"target_type":item.target_type,"evidence":evidence(&item.evidence_json,remaining,depth)?}))).collect::<Result<Vec<Value>>>()?;
    links.sort_by(|a, b| {
        (
            a["occurrence"].as_str(),
            a["target_type"].as_str(),
            a["target_id"].as_str(),
        )
            .cmp(&(
                b["occurrence"].as_str(),
                b["target_type"].as_str(),
                b["target_id"].as_str(),
            ))
    });
    Ok(
        json!({"assets":asset_values,"occurrences":occurrence_values,"observations":observations,"links":links}),
    )
}
