"""The tokens of a file: its comments and how many significant tokens a range holds."""

import bisect
import io
import tokenize
from dataclasses import dataclass

from lang_python.extent import Lines, span
from lang_python.sdk.wire import Comment, Span

SKIPPED = {
    tokenize.COMMENT,
    tokenize.NL,
    tokenize.NEWLINE,
    tokenize.INDENT,
    tokenize.DEDENT,
    tokenize.ENDMARKER,
    tokenize.ENCODING,
}
STRING_STARTS = {getattr(tokenize, name) for name in ("FSTRING_START", "TSTRING_START") if hasattr(tokenize, name)}
STRING_ENDS = {getattr(tokenize, name) for name in ("FSTRING_END", "TSTRING_END") if hasattr(tokenize, name)}


@dataclass(frozen=True)
class CommentToken:
    line: int
    col: int
    """The 0-based byte column of the `#`."""
    end_col: int
    text: str
    alone: bool
    """Whether nothing but blanks precedes it on its line."""


class Tokens:
    """The comments and the significant tokens of a source text, read once."""

    def __init__(self, text: str, lines: Lines):
        self.lines = lines
        self.comments: list[CommentToken] = []
        self.starts: list[tuple[int, int]] = []
        depth = 0
        try:
            for token in tokenize.generate_tokens(io.StringIO(text).readline):
                if token.type == tokenize.COMMENT:
                    self.comments.append(self._comment(token))
                elif token.type in STRING_STARTS:
                    if depth == 0:
                        self.starts.append(self._start(token))
                    depth += 1
                elif token.type in STRING_ENDS:
                    depth = max(0, depth - 1)
                elif token.type not in SKIPPED and depth == 0:
                    self.starts.append(self._start(token))
        except (tokenize.TokenError, SyntaxError):
            pass
        self.by_line = {c.line: c for c in self.comments}

    def _start(self, token: tokenize.TokenInfo) -> tuple[int, int]:
        return token.start[0], self.lines.byte_col(*token.start)

    def _comment(self, token: tokenize.TokenInfo) -> CommentToken:
        line, col = token.start
        alone = token.line[:col].strip() == ""
        return CommentToken(
            line, self.lines.byte_col(line, col), self.lines.byte_col(line, token.end[1]), token.string, alone
        )

    def count(self, start: tuple[int, int], end: tuple[int, int]) -> int:
        """The significant tokens that start in [start, end): comments and layout excluded, a string with
        replacement fields one token."""
        return bisect.bisect_left(self.starts, end) - bisect.bisect_left(self.starts, start)

    def leading(self, line: int) -> CommentToken | None:
        """The first of the comment lines that stand alone right above a line, with no blank line between."""
        first = None
        current = line - 1
        while True:
            comment = self.by_line.get(current)
            if comment is None or not comment.alone:
                return first
            first = comment
            current -= 1

    def trailing(self, line: int, after_col: int) -> CommentToken | None:
        """The comment that follows code on a line, after a column."""
        comment = self.by_line.get(line)
        return comment if comment is not None and not comment.alone and comment.col >= after_col else None

    def grouped(self, symbols_at: dict[int, str]) -> list[Comment]:
        """Every comment, adjacent line comments that stand alone joined, with the symbol each documents."""
        groups: list[list[CommentToken]] = []
        for comment in self.comments:
            previous = groups[-1][-1] if groups else None
            if previous is not None and previous.alone and comment.alone and comment.line == previous.line + 1:
                groups[-1].append(comment)
            else:
                groups.append([comment])
        result: list[Comment] = []
        for group in groups:
            first, last = group[0], group[-1]
            where: Span = span(first.line, first.col, last.line, last.end_col)
            entry: Comment = {"span": where, "text": "\n".join(c.text for c in group)}
            if first.alone and last.line + 1 in symbols_at:
                entry["attached_to"] = symbols_at[last.line + 1]
            result.append(entry)
        return result
