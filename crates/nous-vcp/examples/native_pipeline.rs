//! Offline numerical replay: no model, network or production ANN is invoked.
#[path = "../tests/support/kernel_fixture.rs"]
mod kernel_fixture;
use nous_vcp::{river::readout, thinking::plan};
use serde_json::json;
use std::collections::BTreeMap;

fn main() -> Result<(), String> {
    let (assets, observation) = kernel_fixture::fixture();
    let source_refs = BTreeMap::from([
        (1001, "memory:fixture/alpha@revision-1"),
        (1002, "memory:fixture/omega@revision-1"),
    ]);
    let request = serde_json::from_value(json!({"topK":2,"includeTrace":true,"allowedFileIds":[1],
        "candidates":[{"id":1001,"score":0.8,"bm25Score":0.5},{"id":1002,"score":0.8}]}))
    .map_err(|e| e.to_string())?;
    let ranked = readout(&assets, &observation, request)?;
    let methods = plan(
        &assets,
        &observation,
        serde_json::from_value(json!({
            "stages":[{"diaryName":"fixture","k":2,"candidateIds":[1001,1002]}],"minClosure":0.2
        }))
        .map_err(|e| e.to_string())?,
    )?;
    let refs: Vec<_> = ranked
        .results
        .iter()
        .map(|result| {
            source_refs
                .get(&result.id)
                .map(|source| json!({"candidateId":result.id,"sourceRef":source}))
                .ok_or_else(|| format!("unmapped candidate {}", result.id))
        })
        .collect::<Result<_, _>>()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode":"frozen-numerical-replay", "tagSearch":"exact fixture oracle",
            "generation":assets.generation(),"artifactSig":assets.signature(),
            "assetDiagnostics":assets.diagnostics(),"sourceRefs":refs,
            "observation":observation.observation,"ranking":ranked,"methods":methods
        }))
        .map_err(|e| e.to_string())?
    );
    Ok(())
}
