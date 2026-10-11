# NousQL: Agent manual

Nous stores and retrieves long-term cognition for a Subject. A Subject is the
person or autonomous identity whose knowledge and current work Nous maintains.
A plain Unicode question is already a complete query. Identity selectors, time,
context and exploration are optional ways to make its meaning more precise.

## 1. Objects you need to know

- **Subject**: the identity whose cognition and operating context you query.
  Supply its ID explicitly or select it through the CLI.
- **Cognition**: four durable kinds returned by default: Memory, CognitiveSchema,
  Episode and Journal. A Memory records the Subject’s cognition about a fact or experience; a Schema
  describes a reusable pattern/rule; an Episode organizes an experience; a
  Journal gives a subjective narrative over settled Episodes.
- **Tag**: a durable semantic concept with a stable identity and revisable label
  and description. A Tag can express a topic, rule, experience pattern or a
  sentence-like idea. Its meaning can be embedded for semantic retrieval.
- **LexicalRef**: a stable Agent-readable reference such as
  `mem:zimug-tikub-lulid`. Names discover identities; a returned LexicalRef
  lets you continue using that exact identity. Never invent a LexicalRef.
- **WorkContext**: a bounded current task or question, with a purpose and exact
  cognition references, context text and real Entity/Tag anchors. A Session can
  foreground it once and reuse it for subsequent questions.
- **CognitiveClock**: the Subject's logical clock captured when a query is
  prepared. A live Host may advance it with real time. Replay, simulation and
  tests may set it independently. Relative query time uses this clock.
- **Exploration/diffusion**: bounded propagation from query activation through
  cognition relations to find indirect connections. Request it with `$explore`;
  do not select a physical algorithm.

## 2. Start with plain text

Write a natural-language intent. No Tag, Entity or WorkContext is required.

```nousql
How does Python free-threading work?
```

Text queries use direct lexical and dense retrieval when their Serving families
and embedding capability are available. They do not require graph diffusion,
concept enrichment or a query concept model.

### Prefer explicit referents for retrieval

**Strong recommendation: make known referents explicit before retrieval.** When
the conversation, selected WorkContext, Session or a returned reference identifies
a person, project, object, event or technical concept, put its concrete name and
useful keywords into the query text. This improves lexical term exposure,
embedding semantics and Entity/Tag activation. Preserve the caller's intended
question and uncertainty. If you cannot resolve a referent confidently, query the
original text and let available runtime context contribute. Pronouns, ellipsis
and short follow-up text remain valid queries in every language.

When the task identifies Simon Willison and his blogging practice, prefer
`Why did Simon Willison change his blogging practice later?` to
`Why did he change it later?`. The latter remains executable when its referents
are uncertain. `她为什么修改这个决定？` and `彼は後で何を変更しましたか？` follow
the same rule. Do not invent identities, insert an oracle answer, run a rewriting
model on every turn, or ask for clarification merely to make text eligible.
Exact selectors are optional and must resolve to real identities.

## 3. Resolve identities when they matter

```nousql
deployment approval @e("Alice")
```

```nousql
Recall relevant cognition @tag("active-reader reclamation")
```

An Entity selector is a typed cue. A Tag selector resolves a durable concept and
recalls its direct attachments. Names must resolve uniquely; ambiguity never
turns into a vector guess. Use the chosen candidate's LexicalRef on retry.

```nousql
Inspect this cognition @ref(mem:zimug-tikub-lulid)
```

This example shows syntax only. Replace its illustrative reference with a real
returned reference. `@ref` is an exact read; result projection still applies.
It returns only the bound target, without similarity retrieval, current-context
candidates, concept enrichment, or model reranking. A mutable object reference
selects its head in the current or `$asof` view. `$history` permits eligible prior
revisions but does not turn an exact object read into revision enumeration; use
an immutable revision reference to inspect a particular older version.
Other selectors are cues, not a promise to return that object itself.

## 4. Semantic text is different from a Tag

```nousql
Recall migration decisions #migration
```

```nousql
Recall relevant cognition @tag("KRaft migration")
```

`#migration` contributes semantic concept text without an identity lookup.
`@tag(...)` activates an existing durable Tag. Neither creates a Tag.
Optional Host enrichment may infer existing Tags or ephemeral hypotheses.
It does not authorize a persistent concept mutation.

## 5. Combine intent and preferences

One mandatory unquoted intent is combined with optional `$`, `@` and `#` syntax
islands. `&&`, `||` and parentheses in prose are ordinary text, not operators.
Escape literal island markers with `\$`, `\@`, `\#` and `\\`.

```nousql
Approval and deployment rollback @e("Alice")
```

Hard constraints restrict eligible results. A `$prefer` or `$avoid` preference changes
ranking within eligible results; it does not make a prohibited result eligible.

```nousql
deployment $prefer("rollback") $avoid("draft") $prefer(recent,recorded)
```

A recent preference needs an explicit time axis. Use `$prefer(recent,recorded)` or `$avoid(recent,formed)`.

## 6. Choose the right time axis

| Axis       | Meaning                                                |
| ---------- | ------------------------------------------------------ |
| `occurred` | When the source event happened.                        |
| `observed` | When the Subject received the source evidence.         |
| `valid`    | When the claim applies.                                |
| `formed`   | When cognition or a derived representation was formed. |
| `recorded` | When the canonical revision was recorded.              |

```nousql
deployment incidents $time(occurred,within=30d)
```

`within=30d` is relative to the prepared Subject CognitiveClock. It is unrelated
to network timeouts, process wall-clock deadlines or retry durations.

```nousql
policy state $time(valid,at="2026-05-01T00:00:00Z")
```

Absolute timestamps must carry a timezone. Different axes can be combined;
all constraints must match the same eligible evidence, not different observations.

```nousql
evidence received $time(occurred,from="2026-01-01T00:00:00Z") $time(observed,to="2026-02-01T00:00:00Z")
```

`at` matches an exact instant or an interval containing that point. `from`, `to`
and `within` define a nonempty range: it includes instants inside it and overlaps
intervals. A range is start-inclusive and end-exclusive; equal endpoints are invalid.
Unknown times do not satisfy
known-time constraints. Each axis can appear once in one scope; repeating the
same axis there is an error. `within` cannot mix with `from`, `to` or `at`.

## 7. Ask what was known then

A time filter and an Authority view answer different questions. `$time` filters
chronology; `$asof` chooses the knowledge state used for retrieval.

```nousql
free-threading support status $asof("2025-01-01T00:00:00Z")
```

```nousql
support status $asof(ago=30d)
```

At that cut, names, canonical Tags, revision heads, relations and Serving assets
use the historical state. A future Tag rename, merge or association does not
change the past view. Current permission and purge fences still apply.

## 8. Ask how cognition changed

```nousql
support status $history
```

The default view admits effective heads. `$history` also admits eligible prior
cognition revisions as independent retrieval documents. To exclude later
knowledge while inspecting earlier changes, combine the two controls.

```nousql
support status $history $asof("2025-01-01T00:00:00Z")
```

Five-axis filters still constrain the returned revisions. `$asof` and `$history`
are root-only and apply to the complete query, including identity resolution.

## 9. Direct recall versus associative exploration

```nousql
Recall relevant cognition @tag("active-reader reclamation")
```

This asks for direct Tag attachments. A Tag with no attachments does not
silently start graph diffusion.

```nousql
Recall relevant cognition @tag("active-reader reclamation") $explore
```

```nousql
stale consumer problem $explore
```

These requests explicitly allow bounded associative recall. QueryActivation
unifies text, explicit Tags, optional inferred concepts and permitted context.
An unavailable optional lane is reported; it does not substitute future assets.

## 10. Choose the returned object kinds

Default results are Memory, Schema, Episode and Journal. Evidence and Resource
are explicit choices rather than default top-level results.

```nousql
migration $return(schema)
```

```nousql
migration $return(memory,schema)
```

```nousql
evidence received $return(evidence)
```

`$return(cognition)` explicitly names the default set. Projection is root-only
and also limits exact targets. It does not request a particular physical lane.

## 11. Shape the answer and inspect it

```nousql
migration $limit(10) $diagnostics(full)
```

```nousql
migration $materialize
```

Use `query prepare` to inspect canonical query, captured time, resolved context,
projection and capability requirements before execution. Preparation does not
invoke embedding or concept models. Full execution diagnostics show actual
activation, generations, degradation and the query concept model call count.

## 12. A complete Agent workflow

1. Select the Subject. Preferably expand referents you can identify from current
   work into concrete searchable names and terms. If expansion is uncertain or
   unavailable, query the original text.
2. Resolve Entity/Tag names only when identity matters.
3. On ambiguity, choose a returned candidate LexicalRef and retry.
4. Add the appropriate time axes when the question has chronology.
5. Use `$asof` for “what was known then”; `$history` for prior revisions.
6. Add `$explore` only when indirect associative recall is wanted.
7. Add `$return` only when a particular returned domain is needed.
8. Inspect with `query prepare`, then execute and inspect any degradation.
9. Report actual use of exact returned references with the response `query_id`.
10. Grant bounded concept maintenance only when persistent changes are intended.

CLI examples (the launcher supplies local instance discovery):

```sh
nous help nousql
nous subject use <returned-Subject-lexical-reference>
nous session open
nous context create --purpose "Investigate CPython free-threading" --text "Compare PEP 703, Python 3.13 experimental support, Python 3.14 support and extension compatibility."
nous context foreground
nous identity resolve --kind entity --name CPython
nous context pin --entity <returned-entity-ref>
nous tag search free-threading
nous context pin --tag <returned-tag-ref>
nous query prepare 'When did the support status change?'
nous query 'When did the support status change?'
nous query 'Which extension-compatibility limitations remained?'
nous show result:1
nous trace result:1
nous context pin --cognition result:1
nous use result:1 --kind referenced --event-id <stable-event-id> --occurred-at <timestamp>
```

For a retry, preserve the UseEvent ID, timestamp and consumer identity. `presented`
means exposure only; `referenced`, `acted_on` and `result_supported` are meaningful use.
`result_refuted` is negative feedback, not supporting evidence. Use feedback can
request review; it cannot grant a model or commit a Tag maintenance proposal.

Explicit formation Tags use `nous form <Occurrence-reference> --tag <tag-ref>`. Tag/Association
mutation commands and maintenance grants are listed by `nous help`; use
`--request-file` for CLI-owned semantic TOML revisions and merge/split.

Use `context set --text`, `context pin --cognition result:1`, `context pause|resume`,
`context select` and `context foreground` for reusable task state. Query results
retain real references in query order under `result:N`. Cognition hits retain
their exact immutable revisions for `show`, `trace`, `use` and cognition pin;
Evidence/Resource hits expose the actions their owners support. Mixed results
remain usable even when a source hit cannot receive cognition use. External
resource records appear separately with their stable external identity.
Reuse the selected WorkContext across questions; update its text or anchors only
when the task changes. `CPython`, `PEP 703` and `free-threading` are useful text
keywords; `@tag(...)` activates a real durable concept and `#concept` is an
ephemeral cue. Mutations save a receipt before RPC; `retry <receipt>` reuses
the exact operation identity, inputs and expected revision after an unknown outcome.
Default semantic text uses persistent lexical references for actionable identities,
including Subject, Session, WorkContext and exact Material evidence locators.
Copy returned references into subsequent commands, or use `result:N` within the
consumer's saved query. Unambiguous names and explicit aliases are also supported
within the selected Subject. Names can change or be ambiguous; lexical references
remain stable across restarts and consumers. Source text is preserved verbatim.
`--json` selects the machine envelope with canonical IDs; `--developer` exposes
internal query diagnostics, and `--raw --developer` requests transport DTOs.

## 13. Recover from errors

| Code                             | Next action                                                  |
| -------------------------------- | ------------------------------------------------------------ |
| `UNKNOWN_REFERENCE`              | Resolve a known name or use a returned LexicalRef.           |
| `AMBIGUOUS_REFERENCE`            | Select one returned candidate; do not guess.                 |
| `REFERENCE_TOMBSTONED`           | Rediscover the current active identity.                      |
| `UNRESOLVED_MACHINE_PLACEHOLDER` | Replace an unresolved machine placeholder with actual input. |
| `STALE_CONTEXT`                  | Refresh current WorkContext/session references.              |
| `UNAVAILABLE`                    | Inspect capability and lane diagnostics before retrying.     |

A Tag that did not exist at an as-of cut cannot be used as a past identity.
Future cognition references in current WorkContext are excluded from historical
activation; the current question and purpose remain available.

[Agent entry](README.md) · [Developer implementation reference](../reference/NOUSQL.md)
