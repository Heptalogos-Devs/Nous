use nous_domain::*;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

pub(super) struct Drafts<'a> {
    pub text: &'a str,
    pub units: Vec<ImportTextUnitDraft>,
    pub warnings: Vec<TextImportSplitWarning>,
    text_bytes: usize,
}
impl<'a> Drafts<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            units: vec![],
            warnings: vec![],
            text_bytes: 0,
        }
    }
    pub fn push(
        &mut self,
        start: usize,
        end: usize,
        strategy: ImportTextStrategy,
        context: Value,
    ) -> Result<()> {
        let text = self.text.get(start..end).ok_or_else(|| {
            Error::internal("text splitter produced an invalid UTF-8 source range")
        })?;
        if text.is_empty() {
            return Ok(());
        }
        self.text_bytes = self.text_bytes.saturating_add(text.len());
        if self.units.len() >= crate::import_format::MAX_IMPORT_RECORDS
            || self.text_bytes > crate::import_format::MAX_IMPORT_BYTES
        {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "text split catalog exceeds 4 MiB or 1000 units",
            ));
        }
        let effective = if strategy != ImportTextStrategy::Quote && quote_like(text) {
            ImportTextStrategy::Quote
        } else {
            strategy
        };
        let mut context = context;
        if effective != strategy {
            context["quote_rescue"] = json!(true);
        }
        self.units.push(ImportTextUnitDraft {
            ordinal: self.units.len(),
            start_byte: start as u64,
            end_byte: end as u64,
            text: text.into(),
            effective_strategy: effective,
            context,
            requires_llm: effective != ImportTextStrategy::Quote,
        });
        Ok(())
    }
    pub fn warning(&mut self, code: &str, message: &str, ordinal: Option<usize>) {
        self.warnings.push(TextImportSplitWarning {
            code: code.into(),
            message: message.into(),
            ordinal,
        });
    }
}

pub(super) fn select(text: &str, strategy: ImportTextStrategy) -> ImportTextStrategy {
    if strategy != ImportTextStrategy::Auto {
        return strategy;
    }
    if quote_like(text) {
        ImportTextStrategy::Quote
    } else if factual_like(text) {
        ImportTextStrategy::Factual
    } else {
        ImportTextStrategy::Narrative
    }
}
fn quote_like(text: &str) -> bool {
    static LINES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\r\n|[\n\r\x0b\x0c\x1c-\x1e\x{85}\x{2028}\x{2029}]")
            .expect("static source line regex")
    });
    let lines: Vec<_> = LINES
        .split(text)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines.len() >= 5
        && lines.iter().map(|line| line.chars().count()).sum::<usize>() < lines.len() * 20
}
fn narrative_like(text: &str) -> bool {
    static DIALOGUE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"["「『].*?["」』]"#).expect("static dialogue regex"));
    let prefix: String = text.chars().take(500).collect();
    ["Chapter", "CHAPTER", "###"]
        .iter()
        .any(|marker| prefix.contains(marker))
        || DIALOGUE.is_match(text)
        || [
            "然后",
            "接着",
            "于是",
            "后来",
            "最后",
            "突然",
            "一天",
            "曾经",
            "有一次",
            "从前",
            "说道",
            "问道",
            "想着",
            "觉得",
        ]
        .iter()
        .filter(|marker| text.contains(**marker))
        .count()
            >= 2
}
fn factual_like(text: &str) -> bool {
    let text = text.trim();
    let structured = {
        let parts: Vec<_> = text.split('|').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.trim().is_empty())
    };
    if text.is_empty() || structured || quote_like(text) {
        return false;
    }
    let score = [
        "是",
        "有",
        "在",
        "为",
        "属于",
        "位于",
        "包含",
        "拥有",
        "成立于",
        "出生于",
    ]
    .iter()
    .filter(|marker| text.contains(**marker))
    .count();
    score > 0 && (text.chars().count() <= 240 || (score >= 2 && !narrative_like(text)))
}

pub(super) fn quote(drafts: &mut Drafts<'_>) -> Result<()> {
    let mut start = 0;
    let mut stanza = 0;
    while start <= drafts.text.len() {
        let end = drafts.text[start..]
            .find("\n\n")
            .map_or(drafts.text.len(), |offset| start + offset);
        if !drafts.text[start..end].trim().is_empty() {
            drafts.push(
                start,
                end,
                ImportTextStrategy::Quote,
                json!({"stanza":stanza,"verbatim":true}),
            )?;
        }
        if end == drafts.text.len() {
            break;
        }
        start = end + 2;
        stanza += 1;
    }
    Ok(())
}

pub(super) fn factual(drafts: &mut Drafts<'_>, target: usize) -> Result<()> {
    static STRUCTURAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:[-*]\s+|\d+\.\s+|[^：:]+[：:].+)").expect("static factual line regex")
    });
    let mut start = 0;
    let mut offset = 0;
    let mut count = 0usize;
    let mut pending = false;
    for line in drafts.text.split('\n') {
        let end = offset + line.len();
        count = count.saturating_add(line.chars().count() + 1);
        pending = true;
        let trimmed = line.trim();
        let structural =
            STRUCTURAL.is_match(trimmed) || (trimmed.starts_with('|') && trimmed.ends_with('|'));
        if (count >= target && !structural) || count >= target.saturating_mul(2) {
            drafts.push(start, end, ImportTextStrategy::Factual, json!({}))?;
            start = (end + 1).min(drafts.text.len());
            count = 0;
            pending = false;
        }
        offset = (end + 1).min(drafts.text.len());
    }
    if pending {
        drafts.push(
            start,
            drafts.text.len(),
            ImportTextStrategy::Factual,
            json!({}),
        )?;
    }
    Ok(())
}
