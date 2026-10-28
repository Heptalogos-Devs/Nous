"""Source-preserving strategies adopted from Nous text imports and A-Memorix.

Narrative/chat windows keep source coordinates; factual blocks preserve structures;
quote stanzas remain verbatim and never require a formation model.
"""

import re

from langchain_core.documents import Document
from langchain_text_splitters import RecursiveCharacterTextSplitter
from nous_wave.contracts import DomainError, FreezeCatalog
from pydantic import JsonValue

SCENE = re.compile(r"(?m)^(?:#{1,6}\s+.*|Chapter\s+\d+|\*{3,}$|={3,}$)")
STRUCTURAL = re.compile(r"^(?:[-*]\s+|\d+\.\s+|[^：:]+[：:].+)")
CHAT_TIME = r"(?:\d{4}[-/.]\d{1,2}[-/.]\d{1,2}[ T])?\d{1,2}:\d{2}(?::\d{2})?"
CHAT = re.compile(
    rf"(?m)^(?:\[{CHAT_TIME}\]|{CHAT_TIME})\s*(?:<[^>\r\n]{{1,80}}>|\[[^\]\r\n]{{1,80}}\]|[^\r\n:：]{{1,80}}[：:])\s*"
)


def quote_like(text: str) -> bool:
    lines = [line for line in text.splitlines() if line.strip()]
    return len(lines) >= 5 and sum(map(len, lines)) < len(lines) * 20


def select_strategy(text: str, strategy: str) -> str:
    if strategy != "auto":
        return strategy
    if quote_like(text):
        return "quote"
    score = sum(
        word in text
        for word in ("是", "有", "在", "为", "属于", "位于", "包含", "拥有", "成立于", "出生于")
    )
    narrative = (
        any(marker in text[:500] for marker in ("Chapter", "CHAPTER", "###"))
        or bool(re.search(r'["「『].*?["」』]', text))
        or sum(
            word in text
            for word in (
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
            )
        )
        >= 2
    )
    return (
        "factual"
        if score and (len(text.strip()) <= 240 or score >= 2 and not narrative)
        else "narrative"
    )


def split_text(text: str, options: FreezeCatalog) -> list[Document]:
    if options.strategy_override is None and not options.chat_log:
        return RecursiveCharacterTextSplitter(
            chunk_size=options.chunk_size, chunk_overlap=options.chunk_overlap, add_start_index=True
        ).create_documents([text])
    strategy = (
        "chat_log"
        if options.chat_log
        else select_strategy(text, options.strategy_override or "auto")
    )
    window = options.narrative_window_size or options.chunk_size
    overlap = (
        options.narrative_overlap
        if options.narrative_overlap is not None
        else options.chunk_overlap
    )
    target = options.factual_target_size or options.chunk_size
    if overlap >= window:
        raise DomainError("invalid_split", "Narrative overlap must be smaller than the window")
    result: list[Document] = []

    def push(start: int, end: int, kind: str, context: dict[str, JsonValue]) -> None:
        value = text[start:end]
        if not value.strip():
            return
        effective = "quote" if kind != "chat_log" and quote_like(value) else kind
        result.append(
            Document(
                page_content=value,
                metadata={
                    "start_index": start,
                    "strategy": effective,
                    "requires_llm": effective != "quote",
                    "split_context": context,
                },
            )
        )

    if strategy == "quote":
        cursor = 0
        for ordinal, stanza in enumerate(text.split("\n\n")):
            push(cursor, cursor + len(stanza), "quote", {"stanza": ordinal, "verbatim": True})
            cursor += len(stanza) + 2
        return result
    if strategy == "factual":
        start = offset = count = 0
        pending = False
        for line in text.split("\n"):
            end = offset + len(line)
            count += len(line) + 1
            pending = True
            structural = bool(STRUCTURAL.match(line.strip())) or (
                line.strip().startswith("|") and line.strip().endswith("|")
            )
            if count >= target and not structural or count >= target * 2:
                push(start, end, "factual", {})
                start, count, pending = min(end + 1, len(text)), 0, False
            offset = min(end + 1, len(text))
        if pending:
            push(start, len(text), "factual", {})
        return result
    if strategy == "chat_log":
        starts = [match.start() for match in CHAT.finditer(text)]
        if starts and not text[: starts[0]].strip():
            ends = [*starts[1:], len(text)]
            cursor = 0
            while cursor < len(starts):
                finish, count = cursor, 0
                while finish < len(starts):
                    size = ends[finish] - starts[finish]
                    if finish > cursor and count + size > window:
                        break
                    count += size
                    finish += 1
                    if count >= window:
                        break
                push(
                    starts[cursor],
                    ends[finish - 1],
                    "chat_log",
                    {
                        "message_start": cursor,
                        "message_end": finish,
                        "oversized_message": count > window,
                    },
                )
                if finish == len(starts):
                    return result
                following, retained, index = finish, 0, finish - 1
                while index > cursor and retained < overlap:
                    retained += ends[index] - starts[index]
                    following, index = index, index - 1
                cursor = following
        # Unrecognized chat headers retain all text as narrative, with the reason.
        strategy = "narrative"
        fallback = True
    else:
        fallback = False
    headings = list(SCENE.finditer(text))
    starts = sorted({0, *(match.start() for match in headings)})
    titles = {match.start(): match.group().strip() for match in headings}
    for ordinal, start in enumerate(starts):
        end = starts[ordinal + 1] if ordinal + 1 < len(starts) else len(text)
        cursor, local = start, 0
        while cursor < end:
            finish = min(cursor + window, end)
            if finish < end:
                newline = text.rfind("\n", cursor, finish)
                if newline - cursor > window // 2:
                    finish = newline + 1
            push(
                cursor,
                finish,
                strategy,
                {
                    "scene_id": titles.get(start, "Start"),
                    "scene_ordinal": ordinal,
                    "scene_local_index": local,
                    "chat_log_fallback": fallback,
                },
            )
            if finish == end:
                break
            cursor = max(cursor + 1, finish - overlap)
            local += 1
    return result
