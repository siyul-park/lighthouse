"""Spans, extents and docstrings of declarations."""

import ast
import inspect
from typing import TYPE_CHECKING, Any

from lang_python.symbols import FUNCTION_NODES, Decl
from lang_python.sdk.wire import Position, Span

if TYPE_CHECKING:
    from lang_python.comments import Tokens


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


def start_of(node: ast.AST, lines: Lines) -> tuple[int, int]:
    """Where a statement starts: at its first decorator's `@`, else at the statement itself."""
    decorators = getattr(node, "decorator_list", None)
    if decorators:
        first = decorators[0]
        line = lines.line_text(first.lineno).encode("utf-8")
        at = line.rfind(b"@", 0, first.col_offset)
        return first.lineno, (at if at >= 0 else first.col_offset)
    return node.lineno, node.col_offset  # type: ignore[attr-defined]


def span_for(decl: Decl, lines: Lines) -> Span:
    """The span of a symbol: the whole def or class with its decorators, else the name or statement."""
    node = decl.node
    if isinstance(node, (*FUNCTION_NODES, ast.ClassDef)):
        line, col = start_of(node, lines)
        return span(line, col, node.end_lineno, node.end_col_offset)  # type: ignore[attr-defined]
    return span_of(node)


def extent_of(decl: Decl, tokens: "Tokens", lines: Lines) -> Span | None:
    """The whole declaration: leading comments without a blank line, decorators through the last token,
    an attribute docstring and a comment that trails it."""
    if decl.instance or not decl.statements or decl.statements[0] is None:
        return None
    first = decl.statements[0]
    last = first
    mergeable = decl.kind in ("function", "method", "test") and all(s is not None for s in decl.statements)
    if mergeable and all(isinstance(s, FUNCTION_NODES) for s in decl.statements):
        last = decl.statements[-1]
    line, col = start_of(first, lines)
    lead = tokens.leading(line)
    if lead is not None:
        line, col = lead.line, lead.col
    end_line, end_col = last.end_lineno, last.end_col_offset  # type: ignore[union-attr]
    if decl.doc_statement is not None and last is first:
        end_line, end_col = decl.doc_statement.end_lineno, decl.doc_statement.end_col_offset  # type: ignore[assignment]
    trailing = tokens.trailing(end_line, end_col)
    if trailing is not None:
        end_col = trailing.end_col
    return span(line, col, end_line, end_col)


def doc_of(decl: Decl) -> str | None:
    """The documentation of a declaration: its docstring (PEP 257), or the string that follows a variable."""
    node = decl.node
    if isinstance(node, (*FUNCTION_NODES, ast.ClassDef)):
        return ast.get_docstring(node, clean=True) or None
    statement = decl.doc_statement
    if statement is None:
        return None
    value = statement.value.value  # type: ignore[attr-defined]
    return inspect.cleandoc(value) or None
