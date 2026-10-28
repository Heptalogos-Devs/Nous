use super::{narrative, split::Drafts};
use nous_domain::*;
use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;

pub(super) fn split(drafts: &mut Drafts<'_>, options: &ImportTextSplitOptions) -> Result<()> {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| {
        let time = r"(?:\d{4}[-/.]\d{1,2}[-/.]\d{1,2}[ T])?\d{1,2}:\d{2}(?::\d{2})?";
        Regex::new(&format!(r"(?m)^(?:\[{time}\]|{time})\s*(?:<[^>\r\n]{{1,80}}>|\[[^\]\r\n]{{1,80}}\]|[^\r\n:：]{{1,80}}[：:])\s*")).expect("static chat message regex")
    });
    let text = drafts.text;
    let starts: Vec<_> = HEADER.find_iter(text).map(|found| found.start()).collect();
    if starts.is_empty() || !text[..starts[0]].trim().is_empty() {
        drafts.warning(
            "chat_log_fallback",
            "未识别到完整的时间与发送者消息头，已按普通叙事文本切块",
            None,
        );
        return narrative::split(drafts, options);
    }
    let messages: Vec<_> = starts
        .iter()
        .enumerate()
        .map(|(index, start)| (*start, starts.get(index + 1).copied().unwrap_or(text.len())))
        .collect();
    let sizes: Vec<_> = messages
        .iter()
        .map(|(start, end)| text[*start..*end].chars().count())
        .collect();
    let mut cursor = 0;
    while cursor < messages.len() {
        let mut finish = cursor;
        let mut count = 0usize;
        while finish < messages.len() {
            let candidate = count.saturating_add(sizes[finish]);
            if finish > cursor && candidate > options.narrative_window_size {
                break;
            }
            count = candidate;
            finish += 1;
            if count >= options.narrative_window_size {
                break;
            }
        }
        let ordinal = drafts.units.len();
        drafts.push(
            messages[cursor].0,
            messages[finish - 1].1,
            ImportTextStrategy::ChatLog,
            json!({"scene_id":"Chat Log","message_start":cursor,"message_end":finish}),
        )?;
        if count > options.narrative_window_size && finish == cursor + 1 {
            drafts.warning(
                "oversized_message",
                "消息超过切块窗口，已保留为完整消息",
                Some(ordinal),
            );
        }
        if finish == messages.len() {
            break;
        }
        let mut next = finish;
        let mut overlap = 0usize;
        let mut index = finish - 1;
        while index > cursor && overlap < options.narrative_overlap {
            overlap = overlap.saturating_add(sizes[index]);
            next = index;
            index -= 1;
        }
        cursor = next;
    }
    Ok(())
}
