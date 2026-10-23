use nous_domain::{CognitiveKind, CognitiveRef, EmbeddingInputItem, EmbeddingTarget, MemoryId};

#[test]
fn exact_cognition_identity_and_content_identity_are_independent() {
    let reference = CognitiveRef::memory(&MemoryId::new(), 1);
    let mut input = EmbeddingInputItem {
        target: EmbeddingTarget::Cognition(reference.clone()),
        text: "a stable source projection".into(),
    };
    input.validate().unwrap();
    let key = input.source_key().unwrap();
    let hash = input.source_hash();
    if let EmbeddingTarget::Cognition(reference) = &mut input.target {
        reference.revision += 1;
    }
    assert_ne!(input.source_key().unwrap(), key);
    assert_eq!(input.source_hash(), hash);
    input.text.push_str(" with changed meaning");
    assert_ne!(input.source_hash(), hash);
    assert_eq!(
        input.target.reference().unwrap().kind,
        CognitiveKind::Memory
    );
    assert_eq!(EmbeddingInputItem::tag("  topic  ").unwrap().text, "topic");
    assert!(EmbeddingInputItem::query("  ").is_err());
    assert!(
        serde_json::from_value::<EmbeddingInputItem>(serde_json::json!({
            "target": {"kind":"cognition","reference":reference},
            "text":"valid text", "source_hash":"caller supplied identity"
        }))
        .is_err()
    );
}
