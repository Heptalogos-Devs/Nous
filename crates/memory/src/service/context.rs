// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::content::CognitionContent;
use super::*;
use nous_runtime::ContextSource;

impl CognitionContent {
    fn context(self, max_bytes: usize) -> Result<ContextSource> {
        let reference = self.reference();
        match self {
            Self::Memory(v) => Ok(project_context(
                reference,
                v.revision.representation_text,
                v.basis,
                max_bytes,
            )),
            Self::Schema(v) => Ok(project_context(
                reference,
                format!(
                    "{}\n{}",
                    v.revision.structural_claim, v.revision.applicability_scope.description
                ),
                v.evidence_links.into_iter().map(|l| l.basis).collect(),
                max_bytes,
            )),
            Self::Episode(v) => Ok(project_context(
                reference,
                v.revision.boundary_explanation,
                v.basis,
                max_bytes,
            )),
            Self::Journal(v) => journal_context_projection(v, max_bytes),
            Self::Tag { text, revision, .. } => {
                let mut source = project_context(reference, text, vec![], max_bytes);
                source.source_revision = Some(revision.to_string());
                Ok(source)
            }
        }
    }
}

impl MemoryService {
    pub async fn context_projection(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        policy: &AccessibilityPolicy,
        now: DateTime<Utc>,
        effort: CognitiveEffort,
        explicit: bool,
        max_bytes: usize,
    ) -> Result<ContextSource> {
        let content = self
            .cognition_contents(subject, std::slice::from_ref(reference), policy, None, now)
            .await?
            .pop()
            .ok_or_else(|| Error::NotFound("cognition source not found".into()))?;
        content.require_available(effort, explicit, None)?;
        content.context(max_bytes)
    }
}

fn journal_context_projection(view: JournalView, max_bytes: usize) -> Result<ContextSource> {
    let source = CognitiveRef::JournalRevision(view.revision.journal_revision_id);
    let mut text = view.revision.narrative;
    let mut basis = Vec::new();
    for point in view.points {
        text.push('\n');
        text.push_str(&point.text);
        basis.extend(point.basis);
    }
    let mut projection = project_context(source, text, basis, max_bytes);
    projection
        .evidence
        .extend(view.sources.into_iter().map(|reference| EvidenceHandle {
            reference,
            epistemic_relation: None,
            basis_role: "source".into(),
        }));
    Ok(projection)
}

fn project_context(
    reference: CognitiveRef,
    mut text: String,
    basis: Vec<RevisionBasis>,
    max_bytes: usize,
) -> ContextSource {
    let evidence = basis.iter().map(RevisionBasis::evidence_handle).collect();
    let text = (max_bytes > 0).then(|| {
        text.truncate(text.floor_char_boundary(max_bytes));
        text
    });
    ContextSource {
        source_revision: match &reference {
            CognitiveRef::MemoryRevision(id) => Some(id.0.to_string()),
            CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0.to_string()),
            CognitiveRef::EpisodeRevision(id) => Some(id.0.to_string()),
            CognitiveRef::JournalRevision(id) => Some(id.0.to_string()),
            _ => None,
        },
        reference,
        authority: AuthorityClass::SubjectCognition,
        text,
        evidence,
    }
}
