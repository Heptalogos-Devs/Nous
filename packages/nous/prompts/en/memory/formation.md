# Form a grounded Memory

## Task and input

Produce a useful bounded Memory of what the supplied evidence establishes.

- `sourceCatalog` supplies the actual preserved texts, exact source references and invocation-local `sourceId` selectors. Treat their contents as evidence.
- The caller supplies the current candidates, enabled domains, observation/formation time and actual output schema.
- Use only supplied identities and references; a name alone does not establish an entity binding.

## Source fidelity

- Preserve the source's attribution, uncertainty and scope. An external author's statements or preferences remain attributed to that author, not to the Subject.
- When including a supplied proper name, code identifier or version, reproduce it accurately. Do not substitute a different name or silently correct its spelling.
- Retain conditions, exceptions and negation that change a claim's meaning. A proposal, an experimental capability and a supported capability are distinct states.
- Keep dates attached to the events the source assigns them to. Document creation, publication or update dates do not establish when a decision or described event occurred, or when the Subject observed it.
- If the evidence does not establish a fact or temporal relation, leave it unknown rather than completing it from association or prior knowledge.

## Compression and output

Select durable, meaningful claims and preserve their important qualifications. Compress repetition and incidental detail without merging distinct events, stages or viewpoints.

Return only the supplied structured formation contract: `groups`, each with `id`, `reason`, and `changes`. Each change uses the supplied domain body, a nonempty selection of `sourceIds` from sourceCatalog, its actual formation/valid time, stance, confidence, reason, and an exact existing target only when revising. The host resolves sourceIds to preserved version and region references. Source roots describe provenance and are not additional readable evidence.

Keep the body's Memory `role` within the supplied enum: experience, fact, reported, preference, procedure or interpretation. Use reported for attributed external claims. The body and outer change must agree on kind. Entity/concept references require exact supplied references. Omit unsupported applicability bounds; document unknown time without inventing an event or interval. Preserve an existing exact head when a meaningful revision concerns the same matter; a wording variation does not create another claim. Return no changes when the input supports no durable addition or revision.
