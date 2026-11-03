"""Extract document text while retaining the original and exact page provenance."""

from io import BytesIO
from pathlib import Path

from langchain_core.documents import Document
from nous_sdk.contracts import ImportRecord
from nous.platform.errors import DomainError
from nous_sdk.contracts.imports import TextSplitOptions
from pypdf import PdfReader
from pypdf.errors import PdfReadError
from trafilatura import extract

from .parsing import ParsedRecord, stable_id


def html_records(content: str) -> list[ParsedRecord]:
    text = extract(
        content,
        include_tables=True,
        include_links=True,
        include_comments=False,
        favor_recall=True,
        output_format="txt",
    )
    if not text or not text.strip():
        return [
            ParsedRecord(
                index=0, record=None, error="The HTML document contains no readable main content"
            )
        ]
    return [
        ParsedRecord(
            index=0,
            record=ImportRecord(external_id=stable_id(content), text=text),
            extraction="trafilatura",
        )
    ]


def pdf_records(path: Path | BytesIO) -> list[ParsedRecord]:
    try:
        reader = PdfReader(path)
    except (PdfReadError, ValueError) as error:
        return [ParsedRecord(index=0, record=None, error=f"PDF could not be read: {error}")]
    result: list[ParsedRecord] = []
    for index, page in enumerate(reader.pages):
        text = page.extract_text() or ""
        result.append(
            ParsedRecord(
                index=index,
                record=ImportRecord(external_id=f"page:{index + 1}", text=text)
                if text.strip()
                else None,
                page_number=index + 1,
                extraction="pypdf" if text.strip() else None,
                media_required=not bool(text.strip()),
            )
        )
    if not result:
        raise DomainError("pdf_pages_empty", "The PDF document contains no pages")
    return result


def chunks_for(parsed: ParsedRecord, options: TextSplitOptions) -> list[Document]:
    from .splitting import split_text

    if parsed.media_required:
        return [
            Document(
                page_content="",
                metadata={"start_index": 0, "strategy": "media", "requires_llm": True},
            )
        ]
    if parsed.record is not None and parsed.record.assertions is not None:
        return [
            Document(
                page_content=parsed.record.text,
                metadata={
                    "start_index": 0,
                    "strategy": "record",
                    "requires_llm": False,
                    "split_context": {"record_units": True},
                },
            )
        ]
    return split_text(parsed.record.text, options) if parsed.record else []
