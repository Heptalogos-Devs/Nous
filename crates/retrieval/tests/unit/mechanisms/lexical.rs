// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn natural_language_is_literal_not_search_syntax() {
    let mut index = LexicalGeneration::in_memory().unwrap();
    index
        .add_documents(
            &[LexicalDocument {
                serving_doc_id: 1,
                reference: CognitiveRef::MemoryRevision(nous_core::MemoryRevisionId::new()),
                representation_text:
                    "Apple's on-device language model has three billion parameters".into(),
                title: None,
                entity_refs: Vec::new(),
                tag_ids: Vec::new(),
                schema_ids: Vec::new(),
                source_class: None,
            }],
            crate::MINIMUM_LEXICAL_WRITER_BYTES,
        )
        .unwrap();
    for cue in [
        "How large is Apple's on-device language model?",
        "unknown_field:Apple AND (model",
        "\"Apple's\" +model -language",
    ] {
        let hits = index.search(cue, 10).unwrap();
        assert_eq!(hits.len(), 1, "{cue}");
        assert_eq!(hits[0].serving_doc_id, 1);
    }
    assert!(index.search("?!", 10).unwrap().is_empty());
}
