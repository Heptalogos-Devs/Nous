// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{
    CognitiveQuery, CognitiveQueryExpr, Cue, QueryConstraints, QueryOperation, ResultNeed, TextCue,
};
use nous_subject::{CognitiveSeedInput, CreateSubject};

pub(crate) fn query(subject: nous_core::SubjectId) -> CognitiveQuery {
    CognitiveQuery {
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        subject,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            operation: QueryOperation::Atom,
            targets: Vec::new(),
            preferences: Vec::new(),
            children: Vec::new(),
            cues: vec![Cue::Text(nous_core::TextCue {
                text: "Recall relevant cognition".into(),
            })],
            constraints: QueryConstraints::default(),
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: ResultNeed {
            limit: 32,
            ..Default::default()
        },
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    }
}

pub(crate) async fn subject(runtime: &nous_kernel::NousRuntime) -> nous_core::SubjectId {
    runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: nous_core::OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            },
            metadata: serde_json::json!({}),
            capabilities: None,
        })
        .await
        .expect("subject")
        .subject_id
}

pub(crate) fn text_query(subject: nous_core::SubjectId) -> CognitiveQuery {
    CognitiveQuery {
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        subject,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            operation: QueryOperation::Atom,
            targets: Vec::new(),
            preferences: Vec::new(),
            children: Vec::new(),
            cues: vec![Cue::Text(TextCue {
                text: "diagnostic phrase".into(),
            })],
            constraints: Default::default(),
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: ResultNeed {
            limit: 4,
            ..Default::default()
        },
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    }
}
