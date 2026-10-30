"""The Python fences of host files: each is a module of its own, analysed beside its host and mapped back."""

import posixpath
from dataclasses import dataclass
from typing import Any

from lang_python.paths import SNIPPETS_DIR
from lang_python.wire import Comment, Edge, EmbeddedFile, EmbeddedSource, Fragment, Position, Span, Symbol

UNMAPPED = (-1, -1)


@dataclass
class RegionText:
    """The text a fence is analysed as, with the host bytes each of its bytes came from."""

    text: str
    host: list[tuple[int, int]]
    """For each UTF-8 byte of `text`, the host byte range it stands for, or UNMAPPED."""


@dataclass
class Snippet:
    """One fence as a unit of its own: a file beside its host that uses the project from outside."""

    host: EmbeddedFile
    source: EmbeddedSource
    rel: str
    text: RegionText


def plan_snippets(hosts: list[EmbeddedFile]) -> list[Snippet]:
    """The units of the fences of the host files of a request, in the order of the fences."""
    snippets: list[Snippet] = []
    for host in hosts:
        sources = sorted(host["sources"], key=lambda s: s["source_range"][0])
        for index, source in enumerate(sources):
            directory = posixpath.dirname(host["path"])
            name = "%s.%d" % (posixpath.basename(host["path"]), index + 1)
            rel = posixpath.join(directory, SNIPPETS_DIR, name, "snippet.py")
            snippets.append(Snippet(host, source, rel, region_text(source)))
    return snippets


def region_text(source: EmbeddedSource) -> RegionText:
    """The fence as it is written, each byte at its place in the host."""
    text = source["text"]
    size = len(text.encode("utf-8"))
    start, end = source["source_range"]
    host: list[tuple[int, int]] = [(start, end)] * size
    segments = source.get("text_map")
    if not segments and end - start == size:
        host = [(start + i, start + i + 1) for i in range(size)]
    for segment in segments or []:
        text_from, text_to = segment["text_range"]
        host_from, host_to = segment["source_range"]
        linear = text_to - text_from == host_to - host_from
        for i in range(text_from, min(text_to, size)):
            host[i] = (host_from + i - text_from, host_from + i - text_from + 1) if linear else (host_from, host_to)
    return RegionText(text, host)


def line_starts(data: bytes) -> list[int]:
    return [0] + [i + 1 for i, byte in enumerate(data) if byte == 10]


def position_at(starts: list[int], offset: int) -> Position:
    low, high = 0, len(starts) - 1
    while low < high:
        mid = (low + high + 1) >> 1
        if starts[mid] <= offset:
            low = mid
        else:
            high = mid - 1
    return {"line": low + 1, "col": offset - starts[low] + 1}


def host_line(snippet: Snippet) -> int:
    """The line of the host file a fence starts on."""
    upto = snippet.host["text"].encode("utf-8")[: snippet.source["source_range"][0]]
    return upto.count(b"\n") + 1


class Remap:
    """Puts the spans of the fragment of a fence's file where the host file shows them."""

    def __init__(self, snippet: Snippet, module: str):
        self.snippet = snippet
        self.module = module
        self.lines = line_starts(snippet.text.text.encode("utf-8"))
        self.host_lines = line_starts(snippet.host["text"].encode("utf-8"))

    def fragment(self, source: Fragment) -> Fragment:
        """The fragment as the host shows it, with the host path as its own."""
        dropped: set[str] = set()
        symbols: list[Symbol] = []
        for symbol in source.get("symbols", []):
            where = self.span(symbol["span"])
            if where is None:
                dropped.add(symbol["id"])
                continue
            mapped: Symbol = {**symbol, "span": where, "file": self.snippet.host["path"]}
            if "extent" in symbol:
                extent = self.span(symbol["extent"])
                if extent is None:
                    del mapped["extent"]
                else:
                    mapped["extent"] = extent
            symbols.append(mapped)
        for symbol in symbols:
            if symbol.get("owner") in dropped:
                del symbol["owner"]
        return {
            "file": {"path": self.snippet.host["path"]},
            "modules": source.get("modules", []),
            "symbols": symbols,
            "edges": self.edges(source.get("edges", []), dropped),
            "functions": [f for f in source.get("functions", []) if f["symbol"] not in dropped],
            "tests": [t for t in source.get("tests", []) if t["symbol"] not in dropped],
            "comments": self.comments(source.get("comments", []), dropped),
        }

    def edges(self, edges: list[Edge], dropped: set[str]) -> list[Edge]:
        shown: list[Edge] = []
        for edge in edges:
            if edge["to"] in dropped:
                continue
            mapped: Edge = dict(edge)  # type: ignore[assignment]
            if edge["from"].get("symbol") in dropped:
                mapped["from"] = {"module": self.module}
            if edge.get("site") is not None:
                site = self.span(edge["site"])
                if site is None:
                    del mapped["site"]
                else:
                    mapped["site"] = site
            shown.append(mapped)
        return shown

    def comments(self, comments: list[Comment], dropped: set[str]) -> list[Comment]:
        shown: list[Comment] = []
        for comment in comments:
            where = self.span(comment["span"])
            if where is None:
                continue
            mapped: Comment = {**comment, "span": where}
            if mapped.get("attached_to") in dropped:
                del mapped["attached_to"]
            shown.append(mapped)
        return shown

    def span(self, span: Span) -> Span | None:
        """The host span of a span of the fence's file; None where the host shows nothing of it."""
        start = self.offset(span["start"])
        end = self.offset(span["end"])
        if start is None or end is None:
            return None
        at = self.host_range(start, end)
        if at is None:
            return None
        return {"start": position_at(self.host_lines, at[0]), "end": position_at(self.host_lines, at[1])}

    def offset(self, position: Any) -> int | None:
        line = position["line"] - 1
        if line < 0 or line >= len(self.lines) or position["col"] < 1:
            return None
        return self.lines[line] + position["col"] - 1

    def host_range(self, start: int, end: int) -> tuple[int, int] | None:
        """The host bytes of the bytes start..end: from where the first starts to where the last the host shows ends."""
        host = self.snippet.text.host
        if start >= len(host) or host[start] == UNMAPPED:
            return None
        first = host[start]
        for at in range(min(max(end, start + 1), len(host)) - 1, start - 1, -1):
            if host[at] != UNMAPPED:
                return first[0], max(host[at][1], first[0])
        return None
