use super::*;
use nous_domain::nousql::{Directive, QuerySyntax, time_filter};

pub(super) fn apply(
    leaf: &mut PlannedQuery,
    syntax: &QuerySyntax,
    root: bool,
    clock: &TimePoint,
    recorded: &TimePoint,
    policy: &RetrievalPolicy,
    time_domains: &nous_foundation::clock::domains::TimeDomains,
) -> Result<()> {
    if !syntax.intent.is_empty()
        || syntax
            .cues
            .iter()
            .any(|cue| matches!(cue, QueryCue::Concept { .. }))
        || !leaf.request.input.media.is_empty()
    {
        leaf.direct_only = false;
    } else if leaf.request.input.text.is_empty()
        && syntax
            .cues
            .iter()
            .any(|cue| matches!(cue, QueryCue::Selector { .. }))
    {
        leaf.direct_only = true;
    }
    let mut text = Vec::new();
    if !leaf.request.input.text.is_empty() {
        text.push(leaf.request.input.text.clone());
    }
    if !syntax.intent.is_empty() {
        text.push(syntax.intent.clone());
    }
    for cue in &syntax.cues {
        if let QueryCue::Concept { text: value } = cue {
            text.push(value.clone());
        }
    }
    leaf.request.input.text = text.join(" ");
    leaf.preferences.extend(syntax.preferences.clone());
    let mut filter = QueryFilters::default();
    for directive in &syntax.directives {
        if !root
            && [
                "asof",
                "history",
                "limit",
                "effort",
                "diagnostics",
                "materialize",
            ]
            .contains(&directive.name.as_str())
        {
            return Err(Error::invalid(format!(
                "${} applies to the root query",
                directive.name
            )));
        }
        match directive.name.as_str() {
            "any" | "all" | "asof" => {}
            "history" => {
                directive.flag()?;
                leaf.request.history = true;
                leaf.current_view = false;
            }
            "limit" => {
                let value = directive
                    .positional
                    .first()
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| Error::invalid("$limit requires a positive integer"))?;
                if directive.positional.len() != 1 || !directive.named.is_empty() {
                    return Err(Error::invalid("$limit requires one integer"));
                }
                leaf.request.limit = value;
            }
            "return" => projection(leaf, &directive.strings()?)?,
            "mode" => {
                leaf.request.mode = serde_json::from_value(serde_json::json!(directive.one()?))
                    .map_err(|_| Error::invalid("mode is lexical,dense,vcp or hybrid"))?
            }
            "sort" => {
                if directive.positional.len() != 2
                    || directive.named.keys().any(|key| key != "domain")
                {
                    return Err(Error::invalid(
                        "sort requires time-axis,asc|desc and optional domain",
                    ));
                }
                let axis = match directive.positional[0].as_str() {
                    Some("occurred") => TimeAxis::Event,
                    Some("observed") => TimeAxis::Observation,
                    Some("formed") => TimeAxis::Formation,
                    Some("recorded") => TimeAxis::Recorded,
                    Some("valid") => TimeAxis::Validity,
                    _ => return Err(Error::invalid("unknown sort time axis")),
                };
                let direction = match directive.positional[1].as_str() {
                    Some("asc") => SortDirection::Asc,
                    Some("desc") => SortDirection::Desc,
                    _ => return Err(Error::invalid("sort direction is asc or desc")),
                };
                let domain = directive
                    .named
                    .get("domain")
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| Error::invalid("sort domain is a string"))
                    })
                    .transpose()?;
                leaf.request.sort = Some(QuerySort {
                    axis,
                    direction,
                    domain,
                });
            }
            "source" => {
                filter.source_types = directive
                    .strings()?
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            }
            "exclude" => {
                if !directive.positional.is_empty() || directive.named.len() != 1 {
                    return Err(Error::invalid("use $exclude(source=...)"));
                }
                filter.excluded_source_types.push(
                    directive
                        .named
                        .get("source")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| Error::invalid("use $exclude(source=...)"))?
                        .to_owned(),
                );
            }
            "modality" => {
                filter.modalities = directive
                    .strings()?
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            }
            "cognitiveRole" => filter.cognitive_roles = parse_enum_list(directive)?,
            "formationMode" => filter.formation_modes = parse_enum_list(directive)?,
            "evidenceClass" => filter.evidence_classes = parse_enum_list(directive)?,
            "time" => {
                let ticks =
                    if let Some(within) = directive.named.get("within") {
                        let domain = directive
                            .named
                            .get("domain")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_else(|| {
                                if directive
                                    .positional
                                    .first()
                                    .and_then(serde_json::Value::as_str)
                                    == Some("recorded")
                                {
                                    recorded.domain.as_str()
                                } else {
                                    clock.domain.as_str()
                                }
                            });
                        Some(
                            time_domains.duration_ticks(
                                domain,
                                duration_ms(within.as_str().ok_or_else(|| {
                                    Error::invalid("within requires a duration")
                                })?)?,
                            )?,
                        )
                    } else {
                        None
                    };
                if let Some(filter) = time_filter(directive, clock, recorded, ticks)? {
                    leaf.request.times.push(filter);
                } else {
                    leaf.empty = true;
                }
            }
            "explore" => {
                directive.flag()?;
                leaf.explore = true;
                if matches!(leaf.request.mode, QueryMode::Lexical | QueryMode::Dense) {
                    leaf.request.mode = QueryMode::Vcp;
                }
            }
            "materialize" => {
                directive.flag()?;
                leaf.materialize = true;
            }
            "diagnostics" => {
                let level = directive.one()?;
                if !["none", "summary", "full"].contains(&level) {
                    return Err(Error::invalid("diagnostics is none,summary or full"));
                }
                leaf.diagnostics = level.into();
            }
            "effort" => {
                let effort = directive.one()?;
                if !["light", "normal", "deep", "maximum"].contains(&effort) {
                    return Err(Error::invalid("effort is light,normal,deep or maximum"));
                }
                leaf.effort = effort.into();
            }
            "current" | "authority" => {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "query authority policy requires its cognition authority owner",
                )
                .with_context("directive", directive.name.clone()));
            }
            _ => {
                return Err(Error::invalid(format!(
                    "unsupported directive ${}",
                    directive.name
                )));
            }
        }
    }
    filter.validate()?;
    if let Some(sort) = &mut leaf.request.sort {
        if sort.domain.is_none() {
            sort.domain = Some(
                leaf.request
                    .time
                    .iter()
                    .chain(&leaf.request.times)
                    .find(|time| {
                        std::mem::discriminant(&time.axis) == std::mem::discriminant(&sort.axis)
                    })
                    .map(|time| time.range.domain.clone())
                    .unwrap_or_else(|| {
                        if matches!(sort.axis, TimeAxis::Recorded) {
                            recorded.domain.clone()
                        } else {
                            clock.domain.clone()
                        }
                    }),
            );
        }
        if matches!(sort.axis, TimeAxis::Recorded) && sort.domain.as_deref() != Some("unix_ms") {
            return Err(Error::invalid("recorded sorting uses unix_ms"));
        }
    }
    leaf.candidate_budget = match leaf.effort.as_str() {
        "light" => leaf.request.limit,
        "normal" => policy.candidates(leaf.request.limit),
        "deep" => policy
            .candidates(leaf.request.limit)
            .max(policy.max_candidates / 2),
        "maximum" => policy.max_candidates,
        _ => unreachable!("validated effort"),
    };
    leaf.request.filters = leaf.request.filters.and(&filter);
    policy.admit(&leaf.request)?;
    leaf.request.validate()?;
    Ok(())
}
fn parse_enum_list<T: serde::de::DeserializeOwned>(directive: &Directive) -> Result<Vec<T>> {
    directive
        .strings()?
        .into_iter()
        .map(|value| {
            serde_json::from_value(serde_json::json!(value))
                .map_err(|_| Error::invalid(format!("invalid ${} value", directive.name)))
        })
        .collect()
}
fn projection(leaf: &mut PlannedQuery, names: &[&str]) -> Result<()> {
    let mut domains = Vec::new();
    let mut kinds = Vec::new();
    for name in names {
        match *name {
            "cognition" => {
                if names.len() != 1 {
                    return Err(Error::invalid(
                        "cognition cannot combine with individual return kinds",
                    ));
                }
                domains.push(QueryDomain::Cognition);
            }
            "media" => domains.push(QueryDomain::Media),
            "evidence" | "resource" => {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "this return domain needs its own formal query target",
                )
                .with_context("return_domain", *name));
            }
            name => {
                domains.push(QueryDomain::Cognition);
                kinds.push(
                    serde_json::from_value::<CognitiveKind>(serde_json::json!(name))
                        .map_err(|_| Error::invalid("unknown return kind"))?,
                );
            }
        }
    }
    domains.sort_by_key(|value| match value {
        QueryDomain::Cognition => 0,
        QueryDomain::Media => 1,
    });
    domains.dedup();
    kinds.sort();
    kinds.dedup();
    if leaf.request.domains.is_empty() {
        leaf.request.domains = domains;
    } else {
        leaf.request
            .domains
            .retain(|domain| domains.contains(domain));
        if leaf.request.domains.is_empty() {
            leaf.empty = true;
        }
    }
    if !kinds.is_empty() {
        if leaf.request.kinds.is_empty() {
            leaf.request.kinds = kinds;
        } else {
            leaf.request.kinds.retain(|kind| kinds.contains(kind));
            if leaf.request.kinds.is_empty() {
                leaf.empty = true;
            }
        }
    }
    Ok(())
}
