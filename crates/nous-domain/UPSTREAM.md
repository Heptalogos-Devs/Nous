# Typed cognition sources

`src/structured/` copies and adapts the previous Nous domains at commit
`412b062082afb75ccfae4c3f69fc606d3e3cc862`. The original Apache-2.0 license is
retained in [licenses/Nous-LICENSE](licenses/Nous-LICENSE).

| Original file | Preserved contract |
| --- | --- |
| `crates/core/src/cognition.rs` and `lib.rs` | Exact revision support, positive/counter/context evidence, epistemic classes, source dependence |
| `crates/memory/src/domain.rs` | CognitiveRole, FormationMode, grounded/synthesized support, Association, Schema scope and evidence roles |
| `crates/memory/src/service/tag.rs` | Stable concept meaning, aliases, canonical identity, merge/split lineage and bounded exact inputs |
| `crates/memory/src/service/episode/{mod,partition}.rs` | Tracks, exact ordered members, parent episodes, bounded complete repartition and source lineage |
| `crates/memory/src/service/journal/{mod,mutations}.rs` | Supported narrative points, writing-time stance and knowledge snapshot fence |
| `crates/memory/src/service/schema/{mod,lineage}.rs` | Structural claim, applicability, boundaries, support/counterexamples and refinement lineage |

The adapted types use current UUID identities, revision numbers and named-domain
`CognitiveTimes`. Existing Memory rows remain authoritative; `MemorySemantics`
references an exact Memory revision. All other objects have typed content under
one immutable revision envelope. Each body exposes its complete typed references
and evidence, preserving support, counterevidence and context as separate roles.

Field and structural rules run in the domain. Storage resolves scope, exact
versions, recursive source roots, dependencies, Episode containment and Journal
cutoffs in the existing operation transaction. Caller inputs contain no asserted
independent-root field. Model proposal targets are preallocated slots; source and
output references are checked against the caller's admitted catalog and declared
group dependencies. Independent adoption remains the application owner's task.

`identity`, `sources`, `content`, `objects`, `proposal` and `lineage` own their
respective contracts; `validation` holds the pure structural rules. Shared time
coordinates live in `src/time.rs`. `src/model.rs` owns the common model job
lifecycle and call ledger, while each role retains its own inputs and results.
Embedding inputs use one typed target and text; their content hashes and lookup
keys are derived by the domain.

The typed merge/split/resegmentation requests are executed by
[`nous-app::structured::lineage`](../nous-app/src/structured/lineage.rs) in one
operation transaction. Merge checks every exact head, redirects retired aliases
to the survivor's prior revision, then writes the survivor's union and historical
lineage. Split keeps the broader parent and creates separately supported children.
Resegmentation preserves the complete ordered source membership, track, parent
and time domain; one-to-one reinterpretation keeps its stable identity, while a
changed partition replaces the source organization atomically.
