# Episode Runtime Integration

[返回文档目录](../../INDEX.md)

## Owner

Memory owns Episode service operations; Runtime validates and surfaces exact EpisodeRevision references.

The public Memory service exposes typed Episode create/read/history/list/revise/link/lifecycle operations. Official Client bindings are generated from canonical Proto.

WorkContext records Episode context as an exact `EpisodeRevision` reference. Runtime validates and surfaces it; Memory serves Episode content and lifecycle operations.

Final materialization may expose an exact current EpisodeRevision candidate with its title, boundary explanation and exact provenance. Episode/Journal exact, lexical and dense retrieval use batched Memory-owner final validation and bounded rendering. Runtime meaningful UseEvent accepts their exact revisions; WorkContext also accepts JournalRevision. Automatic organization, Journal and host maintenance follow [Longitudinal Cognition](../cognitive-runtime/longitudinal-cognition.md).
