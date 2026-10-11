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
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        ));
    }
    if query
        .scopes()
        .iter()
        .flat_map(|scope| &scope.cues)
        .any(|cue| matches!(cue, Cue::Text(text) if text.text.contains("<UNRESOLVED:")))
    {
        return Err(nous_core::Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::UnresolvedMachinePlaceholder,
            "Resolve machine placeholders before querying",
        )
        .into());
    }
    Ok(())
}
