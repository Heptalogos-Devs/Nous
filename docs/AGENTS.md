# Nous design and development documents

- (*) `agents/` holds Agent governance documents. An Agent reads and follows them when a requirement says to.
- `design/` holds the design. Record only the current state, not the evolution and not the history.
- `plans/` arranges work requested by the developer or supplied by a spec.
- (*) `sources/` retains the original design discussions. A proposal there is not an approved decision.
- `research/` holds research conclusions and external comparisons. A conclusion can change the implementation. It becomes a contract when a formal spec adopts it.
- `development/` holds operating instructions and implementation records. They follow the current implementation and are not a design contract.
- `examples/` holds configuration and manifest examples, and stays current with the actual format.
- (*) `notes/` holds the developer's notes. Agents only read them.

See [README.md](README.md) for navigation. No Agent may modify a document marked (*) unless the developer has explicitly requested it. When a change is required, produce the patch described by the root file, hand it to the developer for review, and edit the document directly only after authorization.

## Update rules

Define each rule in the topic that owns it. When a decision changes, replace the original text and migrate the related examples.
Do not keep a compatibility note for a retired scheme in the body. CURRENT does not list a discussion archive.
State a rule proposed by research, and a capability the source already has, as separate facts. The scheme chosen in a formal spec is the build contract.

When changing an existing topic, start from the current text in the repository and edit only the paragraphs the new decision affects. Keep a contract the developer added while it remains in force. Update an external spec with a contextual patch, not by replacing the directory.
