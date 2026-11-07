"""只读格式化之后的代码行。空行、注释和文档字符串不计入。"""

from __future__ import annotations

import ast
import io
import tokenize
from collections.abc import Sequence


def _docstring_spans(tree: ast.AST) -> list[tuple[tuple[int, int], tuple[int, int]]]:
    spans: list[tuple[tuple[int, int], tuple[int, int]]] = []

    def visit(node: ast.AST) -> None:
        body: list[ast.stmt] | None
        if isinstance(node, ast.Module):
            body = node.body
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            body = node.body
        else:
            body = None
        if body:
            first = body[0]
            value = first.value if isinstance(first, ast.Expr) else None
            end_line = first.end_lineno
            end_col = first.end_col_offset
            if (
                isinstance(value, ast.Constant)
                and isinstance(value.value, str)
                and end_line is not None
                and end_col is not None
            ):
                spans.append(((first.lineno, first.col_offset), (end_line, end_col)))
        for child in ast.iter_child_nodes(node):
            visit(child)

    visit(tree)
    return spans


def _inside(
    token: tokenize.TokenInfo, spans: Sequence[tuple[tuple[int, int], tuple[int, int]]]
) -> bool:
    return any(span[0] <= token.start and token.end <= span[1] for span in spans)


def python_code_lines(source: str) -> int:
    tree = ast.parse(source)
    spans = _docstring_spans(tree)
    lines: set[int] = set()
    skipped = {
        tokenize.NL,
        tokenize.NEWLINE,
        tokenize.COMMENT,
        tokenize.ENCODING,
        tokenize.INDENT,
        tokenize.DEDENT,
        tokenize.ENDMARKER,
    }
    for token in tokenize.generate_tokens(io.StringIO(source).readline):
        if token.type in skipped or (token.type == tokenize.STRING and _inside(token, spans)):
            continue
        end_line, end_col = token.end
        last = end_line - 1 if end_col == 0 and end_line > token.start[0] else end_line
        lines.update(range(token.start[0], last + 1))
    source_lines = source.splitlines()
    return sum(1 for line in lines if source_lines[line - 1].strip())


def rust_code_lines(source: str) -> int:
    marked: set[int] = set()
    line = 1
    index = 0
    size = len(source)

    def consider(char: str) -> None:
        nonlocal line
        if char == "\n":
            line += 1
        elif not char.isspace():
            marked.add(line)

    def boundary(pos: int) -> bool:
        return pos == 0 or not (source[pos - 1].isalnum() or source[pos - 1] == "_")

    def consume_block(pos: int) -> int:
        nonlocal line
        pos += 2
        depth = 1
        while pos < size and depth:
            if source.startswith("/*", pos):
                depth += 1
                pos += 2
            elif source.startswith("*/", pos):
                depth -= 1
                pos += 2
            else:
                if source[pos] == "\n":
                    line += 1
                pos += 1
        return pos

    def consume_raw(pos: int, hashes: int) -> int:
        closing = '"' + ("#" * hashes)
        while pos < size and not source.startswith(closing, pos):
            consider(source[pos])
            pos += 1
        for char in closing[: max(0, size - pos)]:
            consider(char)
            pos += 1
        return pos

    def consume_string(pos: int) -> int:
        quote = source[pos]
        consider(quote)
        pos += 1
        while pos < size:
            char = source[pos]
            consider(char)
            pos += 1
            if char == "\\" and pos < size:
                consider(source[pos])
                pos += 1
            elif char == quote:
                break
        return pos

    def char_end(pos: int) -> int | None:
        if pos >= size or source[pos] != "'":
            return None
        cursor = pos + 1
        if cursor >= size or source[cursor] in {"\n", "'"}:
            return None
        if source[cursor] == "\\":
            cursor += 1
            if cursor >= size:
                return None
            if source[cursor] == "u" and cursor + 1 < size and source[cursor + 1] == "{":
                cursor += 2
                while cursor < size and source[cursor] != "}":
                    cursor += 1
                cursor += 1
            elif source[cursor] == "x":
                cursor += 3
            else:
                cursor += 1
        else:
            cursor += 1
        if cursor < size and source[cursor] == "'":
            return cursor + 1
        return None

    def mark_between(start: int, end: int) -> None:
        for cursor in range(start, end):
            consider(source[cursor])

    while index < size:
        if source.startswith("//", index):
            while index < size and source[index] != "\n":
                index += 1
            continue
        if source.startswith("/*", index):
            index = consume_block(index)
            continue
        if not boundary(index):
            consider(source[index])
            index += 1
            continue
        raw_at: int | None = None
        raw_hashes = 0
        for prefix in ("br", "cr", "r"):
            if not source.startswith(prefix, index):
                continue
            cursor = index + len(prefix)
            hashes = 0
            while cursor < size and source[cursor] == "#":
                hashes += 1
                cursor += 1
            if cursor < size and source[cursor] == '"':
                raw_at = cursor
                raw_hashes = hashes
                break
        if raw_at is not None:
            mark_between(index, raw_at + 1)
            index = consume_raw(raw_at + 1, raw_hashes)
            continue
        quoted: int | None = None
        for prefix in ("b", "c", ""):
            cursor = index + len(prefix)
            if source.startswith(prefix, index) and cursor < size and source[cursor] == '"':
                quoted = cursor
                break
        if quoted is not None:
            mark_between(index, quoted)
            index = consume_string(quoted)
            continue
        quote_at = index + (1 if source.startswith("b'", index) else 0)
        end = char_end(quote_at) if source.startswith(("'", "b'"), index) else None
        if end is not None:
            mark_between(index, end)
            index = end
            continue
        consider(source[index])
        index += 1
    return len(marked)
