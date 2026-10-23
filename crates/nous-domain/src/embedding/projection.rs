//! Deterministic semantic projection of typed cognition.
use crate::*;

pub fn cognitive_embedding_text(
    value: &StructuredObject,
    base_memory: Option<&Memory>,
) -> Result<String> {
    let lines = match &value.body {
        CognitiveContent::MemorySemantics(content) => {
            let memory = base_memory.ok_or_else(|| {
                Error::invalid("MemorySemantics embedding requires its exact base Memory")
            })?;
            if memory.id != content.memory_id
                || memory.revision != content.memory_revision
                || memory.namespace_id != value.namespace_id
                || memory.subject_id != value.subject_id
            {
                return Err(Error::invalid(
                    "MemorySemantics base differs from its fixed scoped revision",
                ));
            }
            vec![
                memory.title.clone(),
                memory.content.clone(),
                format!("role: {}", serde_json::to_string(&content.cognitive_role)?),
                format!(
                    "formation: {}",
                    serde_json::to_string(&content.formation_mode)?
                ),
                format!("epistemic: {}", serde_json::to_string(&content.epistemic)?),
                content.qualifications.join("\n"),
            ]
        }
        CognitiveContent::Concept(content) => vec![
            content.label.clone(),
            content.description.clone(),
            content.aliases.join("\n"),
        ],
        CognitiveContent::Episode(content) => {
            vec![content.title.clone(), content.boundary_explanation.clone()]
        }
        CognitiveContent::Journal(content) => {
            let mut lines = vec![content.title.clone(), content.narrative.clone()];
            for point in &content.points {
                lines.push(format!(
                    "{}: {}",
                    serde_json::to_string(&point.role)?,
                    point.text
                ));
            }
            lines
        }
        CognitiveContent::Schema(content) => vec![
            content.title.clone(),
            content.claim.clone(),
            content.scope.description.clone(),
            content.boundary.clone(),
        ],
        CognitiveContent::Association(content) => vec![
            content.relation.clone(),
            format!("polarity: {}", serde_json::to_string(&content.polarity)?),
            content.description.clone(),
        ],
    };
    if !matches!(value.body, CognitiveContent::MemorySemantics(_)) && base_memory.is_some() {
        return Err(Error::invalid(
            "only MemorySemantics consumes a base Memory projection",
        ));
    }
    Ok(lines
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}
