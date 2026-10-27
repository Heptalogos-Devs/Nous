use nous_domain::{MediaRecord, QueryResult, RetrievalRecord, media::RepresentationPayload};
use nous_foundation::localization::{FluentArgs, Presentation};
use serde_json::json;
use std::fmt::Write;

pub fn query_result(result: &QueryResult, presentation: &Presentation) -> String {
    let mut output = String::new();
    for hit in &result.hits {
        let _ = write!(
            output,
            "{}  {}={:.4}",
            hit.addresses
                .as_ref()
                .map(|addresses| addresses.exact.to_string())
                .unwrap_or_else(|| hit.reference.to_string()),
            super::label("score", presentation),
            hit.score
        );
        if let Some(score) = hit.rerank_score {
            let _ = write!(
                output,
                "  {}={score:.4}",
                super::label("rerank_score", presentation)
            );
        }
        output.push('\n');
        match &hit.object {
            RetrievalRecord::Memory(memory) => {
                let _ = writeln!(output, "{}\n{}", memory.title, memory.content);
            }
            RetrievalRecord::Structured(value) => {
                output.push_str(&super::render(&json!(value.body), presentation));
                output.push('\n');
            }
            RetrievalRecord::Media(value) => match value {
                MediaRecord::Original(material) => {
                    let _ = writeln!(
                        output,
                        "{}  {} {}",
                        material.media_type,
                        material.byte_len,
                        super::label("bytes", presentation)
                    );
                    if let Some(source) = &material.source {
                        let _ =
                            writeln!(output, "{}: {source}", super::label("source", presentation));
                    }
                    output.push_str(&super::render(&json!(material.metadata), presentation));
                    output.push('\n');
                }
                MediaRecord::SourceRegion(region) => {
                    let _ = writeln!(output, "material:{}", region.material_id);
                    output.push_str(&super::render(&json!(region.coordinate), presentation));
                    output.push('\n');
                }
                MediaRecord::Representation(value) => match &value.payload {
                    RepresentationPayload::Text { text } => {
                        let _ = writeln!(output, "{text}");
                    }
                    RepresentationPayload::Interpretation { value } => {
                        let _ = writeln!(output, "{}", value.summary.content);
                    }
                    RepresentationPayload::Artifact { blob, metadata } => {
                        let _ = writeln!(
                            output,
                            "{}  {} {}",
                            blob.media_type,
                            blob.byte_len,
                            super::label("bytes", presentation)
                        );
                        output.push_str(&super::render(&json!(metadata), presentation));
                        output.push('\n');
                    }
                },
                MediaRecord::DerivedRegion(region) => {
                    let _ = writeln!(
                        output,
                        "representation:{}@{}",
                        region.representation.id, region.representation.revision
                    );
                    output.push_str(&super::render(&json!(region.selector), presentation));
                    output.push('\n');
                }
            },
        }
        let _ = writeln!(
            output,
            "{}: {}\n",
            super::label("source_roots", presentation),
            hit.source_roots
                .iter()
                .map(|id| hit
                    .source_addresses
                    .get(id)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("material:{id}")))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let mut args = FluentArgs::new();
    args.set("count", result.hits.len());
    args.set(
        "complete",
        super::label(
            if result.coverage.complete {
                "yes"
            } else {
                "no"
            },
            presentation,
        ),
    );
    args.set("indexed", result.coverage.indexed_through);
    args.set("authority", result.coverage.authority_commit_seq);
    if let Ok(summary) = presentation.message("query-summary", Some(&args)) {
        let _ = writeln!(output, "{summary}");
    }
    for issue in &result.coverage.projection_issues {
        let _ = writeln!(
            output,
            "{}: {}",
            issue.target,
            super::error_text(&issue.error, presentation)
        );
    }
    output.trim_end().to_owned()
}
