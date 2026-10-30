"""Lines, columns and spans as the protocol counts them: 1-based, columns in UTF-8 bytes, ends exclusive."""

import ast
from typing import Any

from lang_python.wire import Position, Span


def position(line: int, col: int) -> Position:
    return {"line": line, "col": col}


def span(start_line: int, start_col: int, end_line: int, end_col: int) -> Span:
    """A span from 1-based lines and 0-based byte columns, as `ast` counts them."""
    return {"start": position(start_line, start_col + 1), "end": position(end_line, end_col + 1)}


def span_of(node: ast.AST) -> Span:
    return span(node.lineno, node.col_offset, node.end_lineno, node.end_col_offset)  # type: ignore[attr-defined]


class Lines:
    """The lines of a text, for converting character columns to byte columns."""

    def __init__(self, text: str):
        self.text = text
        self.lines = text.split("\n")

    def byte_col(self, line: int, char_col: int) -> int:
        """The 0-based byte column of a 0-based character column on a 1-based line."""
        if line - 1 >= len(self.lines):
            return char_col
        return len(self.lines[line - 1][:char_col].encode("utf-8"))

    def line_text(self, line: int) -> str:
        return self.lines[line - 1] if 0 < line <= len(self.lines) else ""

    def span_chars(self, start: tuple[int, int], end: tuple[int, int]) -> Span:
        """A span from (line, character column) pairs, as `tokenize` counts them."""
        return span(start[0], self.byte_col(*start), end[0], self.byte_col(*end))


def before(a: Any, b: Any) -> bool:
    """Whether the (line, column) of a comes before that of b."""
    return (a["line"], a["col"]) < (b["line"], b["col"])
