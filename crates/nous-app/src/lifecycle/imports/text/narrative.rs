use super::split::Drafts;
use nous_domain::*;
use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;

pub(super) fn split(drafts: &mut Drafts<'_>, options: &ImportTextSplitOptions) -> Result<()> {
    static SCENE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^(?:#{1,6}\s+.*|Chapter\s+\d+|\*{3,}$|={3,}$)")
            .expect("static scene regex")
    });
    let text = drafts.text;
    let mut cursor = 0;
    let mut title = "Start";
    let mut scene = 0;
    let mut heading = None;
    for found in SCENE.find_iter(text) {
        if cursor < found.start() && !(cursor == 0 && text[..found.start()].trim().is_empty()) {
            window(
                drafts,
                cursor,
                found.start(),
                title,
                scene,
                heading,
                options,
            )?;
            scene += 1;
        }
        title = found.as_str().trim();
        heading = Some((found.start(), found.end()));
        cursor = found.end();
    }
    if cursor < text.len() {
        window(drafts, cursor, text.len(), title, scene, heading, options)?;
    } else if drafts.units.is_empty() {
        drafts.warning(
            "scene_without_body",
            "场景标题没有正文，已保留完整原文",
            None,
        );
        window(drafts, 0, text.len(), "Whole Text", 0, None, options)?;
    }
    Ok(())
}

fn window(
    drafts: &mut Drafts<'_>,
    begin: usize,
    end: usize,
    title: &str,
    scene: usize,
    heading: Option<(usize, usize)>,
    options: &ImportTextSplitOptions,
) -> Result<()> {
    let text = drafts.text;
    let mut boundaries: Vec<_> = text[begin..end]
        .char_indices()
        .map(|(index, _)| begin + index)
        .collect();
    boundaries.push(end);
    let length = boundaries.len() - 1;
    let mut start = 0;
    let mut local = 0;
    while start < length {
        let mut finish = start
            .saturating_add(options.narrative_window_size)
            .min(length);
        if finish < length {
            let chunk = &text[boundaries[start]..boundaries[finish]];
            if let Some(newline) = chunk.rfind('\n') {
                let prefix_chars = chunk[..newline].chars().count();
                if prefix_chars > options.narrative_window_size / 2 {
                    finish = start + prefix_chars + 1;
                }
            }
        }
        drafts.push(boundaries[start], boundaries[finish], ImportTextStrategy::Narrative,
            json!({"scene_id":title,"scene_ordinal":scene,"scene_local_index":local,
                "heading_range":heading.map(|(start,end)|json!({"start_byte":start,"end_byte":end}))}))?;
        if finish == length {
            break;
        }
        start += (finish - start)
            .saturating_sub(options.narrative_overlap)
            .max(1);
        local += 1;
    }
    Ok(())
}
