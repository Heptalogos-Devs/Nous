// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use nous_core::*;

pub(super) fn validate_query_input(query: &CognitiveQuery) -> Result<()> {
    query.validate()?;
    if !query
        .scopes()
        .iter()
        .flat_map(|scope| &scope.cues)
        .any(|cue| matches!(cue, Cue::Text(text) if !text.text.trim().is_empty()))
    {
        return Err(Error::Invalid(
            "query requires nonempty intent TextCue".into(),
        ));
    }
    if query
        .scopes()
        .iter()
        .flat_map(|scope| &scope.cues)
        .any(|cue| matches!(cue, Cue::Text(text) if text.text.contains("<UNRESOLVED:")))
    {
        return Err(DomainError::new(
            DomainErrorCode::UnresolvedMachinePlaceholder,
            "Resolve machine placeholders before querying",
        )
        .into());
    }
    Ok(())
}
