use super::*;
use crate::{Error, ErrorCode, Result};
use winnow::{ModalResult, Parser, token::take_while};

fn word<'a>(input: &mut &'a str) -> ModalResult<&'a str> {
    take_while(1.., |c: char| c.is_ascii_alphanumeric() || c == '_').parse_next(input)
}
fn bare<'a>(input: &mut &'a str) -> ModalResult<&'a str> {
    take_while(1.., |c: char| {
        !c.is_whitespace() && !['(', ')', ',', '='].contains(&c)
    })
    .parse_next(input)
}
fn scalar(input: &mut &str) -> Result<Value> {
    *input = input.trim_start();
    if input.starts_with('"') {
        let mut decoder = serde_json::Deserializer::from_str(input).into_iter::<String>();
        let value = decoder
            .next()
            .ok_or_else(|| Error::invalid("missing quoted argument"))?
            .map_err(|error| Error::invalid("invalid quoted argument").with_cause(&error))?;
        *input = &input[decoder.byte_offset()..];
        return Ok(Value::String(value));
    }
    let value = bare
        .parse_next(input)
        .map_err(|_| Error::invalid("invalid NousQL argument"))?;
    Ok(serde_json::from_str::<serde_json::Number>(value)
        .map(Value::Number)
        .unwrap_or_else(|_| Value::String(value.into())))
}
fn arguments(input: &mut &str) -> Result<(Vec<Value>, BTreeMap<String, Value>)> {
    let mut positional = Vec::new();
    let mut named = BTreeMap::new();
    *input = input
        .strip_prefix('(')
        .ok_or_else(|| Error::invalid("expected argument list"))?;
    *input = input.trim_start();
    if let Some(rest) = input.strip_prefix(')') {
        *input = rest;
        return Ok((positional, named));
    }
    loop {
        let before = *input;
        let first = scalar(input)?;
        *input = input.trim_start();
        if let Some(rest) = input.strip_prefix('=') {
            let key = first
                .as_str()
                .filter(|key| key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .ok_or_else(|| Error::invalid("invalid named argument"))?
                .to_owned();
            *input = rest;
            if named.insert(key, scalar(input)?).is_some() {
                return Err(Error::invalid("duplicate named argument"));
            }
        } else {
            if !named.is_empty() {
                return Err(Error::invalid("positional argument follows named argument"));
            }
            positional.push(first);
        }
        if input.len() == before.len() {
            return Err(Error::internal("NousQL argument parser made no progress"));
        }
        *input = input.trim_start();
        if let Some(rest) = input.strip_prefix(')') {
            *input = rest;
            break;
        }
        *input = input
            .strip_prefix(',')
            .ok_or_else(|| Error::invalid("argument list requires comma or closing parenthesis"))?
            .trim_start();
        if positional.len() + named.len() > 128 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "NousQL island has too many arguments",
            ));
        }
    }
    Ok((positional, named))
}
fn directive(output: &mut QuerySyntax, value: Directive) -> Result<()> {
    if matches!(value.name.as_str(), "prefer" | "avoid") {
        let args = value.strings()?;
        let preference = match args.as_slice() {
            [text] if !text.trim().is_empty() => QueryPreference {
                negative: value.name == "avoid",
                text: Some((*text).into()),
                recent: None,
            },
            ["recent", axis]
                if ["occurred", "observed", "formed", "recorded", "valid"].contains(axis) =>
            {
                QueryPreference {
                    negative: value.name == "avoid",
                    text: None,
                    recent: Some((*axis).into()),
                }
            }
            _ => {
                return Err(Error::invalid(
                    "preference requires text or recent,time-axis",
                ));
            }
        };
        output.preferences.push(preference);
        return Ok(());
    }
    if ![
        "return",
        "asof",
        "history",
        "limit",
        "source",
        "modality",
        "time",
        "exclude",
        "explore",
        "effort",
        "diagnostics",
        "materialize",
        "cognitiveRole",
        "formationMode",
        "evidenceClass",
        "authority",
        "current",
        "any",
        "all",
        "mode",
        "sort",
    ]
    .contains(&value.name.as_str())
    {
        return Err(Error::invalid(format!(
            "unknown NousQL directive ${}",
            value.name
        )));
    }
    let key = |item: &Directive| {
        if item.name == "time" {
            format!("time:{}", item.positional.first().unwrap_or(&Value::Null))
        } else {
            item.name.clone()
        }
    };
    if value.name != "exclude"
        && output
            .directives
            .iter()
            .any(|item| key(item) == key(&value))
    {
        return Err(Error::invalid(format!(
            "duplicate NousQL directive ${}",
            value.name
        )));
    }
    output.directives.push(value);
    Ok(())
}
/// Natural-language mode switches only at explicit, boundary-delimited syntax islands.
pub fn parse(source: &str) -> Result<QuerySyntax> {
    if source.len() > 32768 {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "NousQL source exceeds 32 KiB",
        ));
    }
    let mut output = QuerySyntax {
        intent: String::new(),
        cues: Vec::new(),
        directives: Vec::new(),
        preferences: Vec::new(),
    };
    let mut input = source;
    let mut previous = None;
    let mut islands = 0;
    while !input.is_empty() {
        let current = input.chars().next().expect("nonempty text");
        let rest = &input[current.len_utf8()..];
        if current == '\\'
            && rest
                .chars()
                .next()
                .is_some_and(|next| "$@#\\".contains(next))
        {
            let escaped = rest.chars().next().expect("escaped character");
            output.intent.push(escaped);
            input = &rest[escaped.len_utf8()..];
            previous = Some(escaped);
            continue;
        }
        let boundary = previous.is_none_or(|c: char| !c.is_alphanumeric() && c != '_' && c != '\\');
        if boundary && ['@', '$'].contains(&current) {
            let mut after = rest;
            if let Ok(name) = word.parse_next(&mut after) {
                let selector = SelectorKind::parse(name);
                if (current == '$'
                    && name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_'))
                    || (selector.is_some() && after.starts_with('('))
                {
                    let (positional, named) = if after.starts_with('(') {
                        arguments(&mut after)?
                    } else {
                        (Vec::new(), BTreeMap::new())
                    };
                    if current == '$' {
                        directive(
                            &mut output,
                            Directive {
                                name: name.into(),
                                positional,
                                named,
                            },
                        )?;
                    } else {
                        let kind = selector.expect("recognized selector");
                        if !named.is_empty()
                            || positional.is_empty()
                            || positional.len() > 32
                            || (kind != SelectorKind::Entity && positional.len() != 1)
                        {
                            return Err(Error::invalid("invalid selector arity"));
                        }
                        let locators = positional
                            .into_iter()
                            .map(|value| {
                                value
                                    .as_str()
                                    .filter(|value| !value.trim().is_empty())
                                    .map(str::to_owned)
                                    .ok_or_else(|| {
                                        Error::invalid(
                                            "selectors require nonempty names or exact references",
                                        )
                                    })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        output.cues.push(QueryCue::Selector {
                            selector: kind,
                            locators,
                        });
                    }
                    input = after;
                    previous = Some(')');
                    islands += 1;
                    continue;
                }
            }
        }
        if boundary && current == '#' {
            let mut after = rest;
            let concept = if after.starts_with('(') {
                let (args, named) = arguments(&mut after)?;
                if args.len() != 1 || !named.is_empty() {
                    return Err(Error::invalid("concept requires one text argument"));
                }
                args[0].as_str().map(str::to_owned)
            } else {
                let end = after
                    .char_indices()
                    .take_while(|(_, c)| c.is_alphanumeric() || ['_', '-'].contains(c))
                    .map(|(offset, c)| offset + c.len_utf8())
                    .last()
                    .unwrap_or(0);
                if end == 0 {
                    None
                } else {
                    let text = after[..end].to_owned();
                    after = &after[end..];
                    Some(text)
                }
            };
            if let Some(text) = concept {
                if text.trim().is_empty() {
                    return Err(Error::invalid("empty concept"));
                }
                output.cues.push(QueryCue::Concept { text });
                input = after;
                previous = Some(')');
                islands += 1;
                continue;
            }
        }
        output.intent.push(current);
        previous = Some(current);
        input = rest;
        if islands > 2048 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "NousQL syntax-island limit exceeded",
            ));
        }
    }
    output.intent = output
        .intent
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if islands > 2048 {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "NousQL syntax-island limit exceeded",
        ));
    }
    Ok(output)
}
