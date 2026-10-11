# Proquint reference migration — Issue 23

[返回文档目录](../INDEX.md)

[Issue 23](https://github.com/Heptalogos-Devs/Nous-Wave/issues/23) selects standard
[Proquint](https://arxiv.org/html/0901.4016): three MSB-first 16-bit CVCVC groups,
the standard consonant/vowel alphabets, and no optional prefix. The body is
17 characters; a `mem:` address is 21. Canonical object identity and ownership
do not change. This is a secondary address design improvement, not evidence
of improved embeddings, tokenizer cost, retrieval quality or semantic neutrality.

## Durable inventory

| Location | What is stored | Migration treatment |
| --- | --- | --- |
| `lexical_bindings` | Address primary key, kind, canonical identity, creation time, tombstone | Replace the key through an explicit complete mapping; preserve every other identity/state field. Remove the superseded wordlist-version column. |
| `lexical_visibility` | Address foreign key, Subject, display name, aliases | Replace only the address; preserve scope and labels. |
| `authority_object_states`, kind `lexical_visibility` | Historical address in `object_ref` and `state.lexical_ref` | Replace both structured address fields, preserving event identity, timestamps, historical name/alias values and Subject. Avoid emitting fictional new historical visibility events. |
| Historical read snapshots and Serving | Rebuildable descriptors/digest derived from the above | Rebuild from migrated Authority; do not substitute new cognition. |
| CLI selection/result state | Canonical Subject/Session/WorkContext IDs and typed canonical hit refs | No address-key rewrite is needed. Verify the recovered consumer still selects the same objects. |
| CLI receipts | Canonical RPC requests, operation identity and expected revisions; user-authored content may contain addresses | Preserve mutation identity and content; do not blindly replace strings inside requests. |
| Artifact bytes, immutable cognition revisions, model workflow snapshots | Original source/proposal content, potentially containing literal old addresses | Preserve bytes and immutable content. Retain the complete old/new/canonical mapping in the migration record; do not add an old-format consumer parser or silently alter provenance. |
| Mutable WorkContext text and local research checkpoints | Host-authored task text and references; typed anchors remain canonical | Inspect actual occurrences and update active handoff text explicitly through the owner when needed; do not treat arbitrary prose as a structured reference column. |
| Current parser/CLI/MCP help, fixtures and maintained documents | Operative address syntax/examples | Update together to the new format and remove the old vocabulary producer/validator and its measurement tooling. Original operation evidence under ignored research data remains historical evidence. |

The replaced allocator ordered its four 12-bit word indices least-significant
first. A migration mapping recovers those original 48 bits and applies the new
MSB-first Proquint encoding. This gives a deterministic one-to-one conversion
for valid existing entries; it does not claim that future random allocation is
collision-free. New allocations retain transaction-local uniqueness checks
and bounded retries.

## One-time offline migration

This is an explicit conversion of the current research databases. Production
contains only the standard codec and current schema, with no legacy parser,
runtime migration module, compatibility mode or automatic old-address alias.
The one-time script, backup and mapping are ignored research operation evidence.

Stop producers for the affected instance. Capture the complete Directory and
historical visibility inventory, original vocabulary identity and a mapping
containing old address, new address, kind and canonical identity. Reject
unmapped or malformed entries and duplicate destination keys before changing
Authority. Preserve a verifiable pre-migration backup and the mapping record.

In one database transaction, lock the affected tables, verify the exact expected
old schema/migration checksum, replace the Directory primary/foreign keys and
the two historical visibility address fields, and remove the old schema shape.
Preserve canonical IDs, Subject visibility, labels, tombstones, timestamps and
historical event IDs. Suppress only the visibility-capture trigger while doing
this mechanical address migration; re-enable it before committing. Reconcile
the recorded foundation checksum only after the resulting schema has been
verified equivalent to the new fresh schema. A rerun must verify the already
migrated bindings against the same manifest and perform no reallocation.

After deployment, use returned new addresses and unchanged canonical anchors
to resume the original research tasks, including exact/history/source reads,
ambiguity and tombstone behavior. Address literals in original source prose
remain historical text, not operative address metadata. The migration record
preserves their correspondence without creating a permanent compatibility
resolver. Any unsupported durable location or missing mapping must stop the
transaction and be reported rather than discarded.

The producer, consumer and fresh schema now use only standard Proquint. The
superseded vocabulary and measurement files have been deleted. All four research databases with durable cognition were backed up and converted
offline. The two active instances retained their original users and task state: `dogfooding` retained
131 bindings, 131 visibility rows and 134 historical visibility rows;
`dogfooding-portable` retained 51 of each. Full before/after records were compared,
including nested historical labels, aliases, tombstones, canonical identities
and event timestamps. A second execution verified the same mappings without
reallocation. The main Core has restarted on the new schema; the existing CLI
consumer recovered the same Subject, WorkContext and four cognition anchors
through their new addresses. A complete source-less Portable installation was
then deployed against the original Authority and identity roots. Its original
user recovered the same Subject/WorkContext/seven anchors, completed both
current and as-of correction audits, read the original 1220/3919-byte sources
without truncation, and resolved the original display name and a newly useful
alias. The active checkpoint now uses new addresses; the new Session is closed.


The remaining durable instances were converted with the same complete procedure:
`core-cognition-v2` retained 135 bindings, 139 visibility rows, 139 historical
visibility rows and 12 Subjects; `dogfooding-disposable` retained 13 of each
address/visibility/history row and one Subject. Their nested before/after
records and repeat execution were verified against the original mapping,
without reallocation. The `dev` and `portable` locations are locator templates
with no persistent database to convert. Backups and full inventories for all
four instances remain under ignored `data/research/runs/full-system-dogfooding/proquint-migration/`; the unused instances were not made active consumers.

Actual database uniqueness conflicts were exercised through the unchanged owner
transaction: two forced conflicts followed by successful allocation, and32
consecutive conflicts followed by the explicit retry-bound error and rollback.
The resident canonical identity, display name and aliases remained intact; no
partial exhausted binding or visibility was published. Repeated address-only
bind reused the new standard reference without another allocation. This
regression uses the existing real database host and adds no production RNG or
compatibility interface.
