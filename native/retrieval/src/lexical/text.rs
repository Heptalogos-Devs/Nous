//! Shared lexical analysis and Tantivy schema.
use super::TOKENIZER;
use std::collections::HashSet;
use tantivy::{
    schema::{IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions},
    tokenizer::{LowerCaser, RemoveLongFilter, TextAnalyzer, TokenStream},
};
use tantivy_jieba::JiebaTokenizer;

/// Token overlap for a bounded authority delta, using the exact same Chinese/Latin analyzer.
/// Index BM25 and this local score share matching semantics; the app owns rank fusion.
pub fn match_score(query: &str, text: &str) -> Option<f32> {
    let query = tokenize(query);
    if query.is_empty() {
        return None;
    }
    let document = tokenize(text);
    let matched = query.intersection(&document).count();
    (matched > 0).then_some(matched as f32 / query.len() as f32)
}

pub(super) fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(JiebaTokenizer::new())
        .filter(RemoveLongFilter::limit(80))
        .filter(LowerCaser)
        .build()
}

pub(super) fn tokenize(text: &str) -> HashSet<String> {
    let mut analyzer = analyzer();
    let mut stream = analyzer.token_stream(text);
    let mut terms = HashSet::new();
    while stream.advance() {
        let text = &stream.token().text;
        if text.chars().any(char::is_alphanumeric) {
            terms.insert(text.clone());
        }
    }
    terms
}

pub(super) fn schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("id", STRING | STORED);
    builder.add_u64_field("group", tantivy::schema::FAST);
    builder.add_text_field("namespace", STRING);
    builder.add_text_field("subject", STRING);
    builder.add_text_field(
        "body",
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqs),
        ),
    );
    builder.add_text_field(
        "title",
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqs),
        ),
    );
    builder.build()
}
