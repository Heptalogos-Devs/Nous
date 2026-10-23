# Original media and derivation protocol

The prepared domain module is `src/media.rs`, with focused modules for locators,
derivation, interpretation and sampling facts. The public contract is
`nous_domain::media`; `tests/media.rs` exercises that same exported module.

Existing `Material` remains the original object's identity and stores the actual
content hash, MIME type and byte length. `PutMediaRequest` contains upload
metadata and optional expected length/hash. App receives the binary body as a
stream into its object store, checks the received facts, and publishes the normal
Material row. Binary reading and text decoding use separate App interfaces.

`SourceRegion` binds typed pixel, frame/time, page or byte coordinates to one
Material. Its parent must contain the same original region. `DerivedRegion`
binds text offsets, a typed interpretation item, or artifact coordinates to an
exact representation revision. Neither type accepts an asserted independent root.
The source owner follows their exact edges back to original Materials.

`DerivedRepresentation` records ordered unique inputs, the actual producer
revision, prompt/schema/config/preprocessing digests, physical channel access, content and
supersession. Its result can be text, a bounded typed `MediaInterpretation`, or
an object-store blob. Derived blobs have content identities and lineage inputs;
they do not create fresh independent original-material roots.

The three strategies preserve their upstream meaning:

- `description_only` produces a committed description or transcription.
- `direct_structured` requires actual original source input.
- `describe_then_structure` uses committed descriptions/transcripts and records
  represented visual/audio access in the second stage.

`MediaInterpretation` is a material understanding result. It contains bounded
source-linked observations, text/speech, mentions, interpretations and uncertainty.
Observed coverage and direct basis require original input; a description cannot
upgrade them. Cognition adoption continues through the separate typed structured
formation contract.

Input access records the producer's actual physical input. `MediaExposure`
qualifies an existing Contact by its exact delivered locator and access, so the
consumer can distinguish receiving pixels/audio from receiving their description.
Background processing does not create Contact. Storage and delivery owners supply
resolved roots and actual access when validating this attachment.

The next implementation connects streamed originals, region/representation
storage, exact EvidenceSource media locators, model resource routes and bounded
preprocessing. Existing Material/Observation/structured evidence wire formats are
unchanged in this prepared module.

Sources are locked to previous Nous commit
`412b062082afb75ccfae4c3f69fc606d3e3cc862`. The actual files were copied to
`temp/spikes/media-research` before adaptation. [media-sources.json](media-sources.json)
records original hashes, source paths and module mappings. Their Apache-2.0
license is retained in [licenses/Nous-LICENSE](licenses/Nous-LICENSE).
