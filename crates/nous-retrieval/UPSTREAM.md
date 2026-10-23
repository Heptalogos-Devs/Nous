# Reused source

The index schema, document replacement, literal natural-language term queries,
scope filters and TopDocs materialization in `src/lib.rs` are adapted from
[`crates/retrieval/src/mechanisms/lexical.rs`](https://github.com/Heptalogos-Devs/Nous-Wave/blob/412b062082afb75ccfae4c3f69fc606d3e3cc862/crates/retrieval/src/mechanisms/lexical.rs)
at commit `412b062082afb75ccfae4c3f69fc606d3e3cc862`. Its Apache-2.0 license is
retained in [licenses/Nous-LICENSE](licenses/Nous-LICENSE).

Tantivy 0.27.0's `PreparedCommit::set_payload` commits index coverage with the
projection in the same atomic metadata update. Its implementation was inspected
from the local upstream checkout at commit
`67889269cae67063c372bc236b102d632a021568`.

Chinese and Latin tokenization uses the `tantivy-jieba` 0.21 dependency directly,
with Tantivy's lowercase and long-token filters. The bridge was inspected from
the local upstream checkout at commit
`882dcc4304e415879b9e0169ff439a4806a2bf8e`. Its tokenizer API 0.7 matches Tantivy
0.27; no tokenizer implementation is copied into Nous.
