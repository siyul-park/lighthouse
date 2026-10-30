"""The fragment of one source file: its symbols, edges, function summaries, tests and comments."""

import json
from dataclasses import dataclass

from lang_python.symbols import MODULE_BODY, Decl
from lang_python.extent import doc_of, extent_of, span_for
from lang_python.generated import is_generated
from lang_python.project import Model, ParsedFile
from lang_python.project import SNIPPETS_DIR
from lang_python.modules import directory_of, tested_module
from lang_python.tests import is_test_file
from lang_python.extent import span_of
from lang_python.project import Resolver, resolver_of
from lang_python.project import Scope
from lang_python.tests import role_of, test_case
from lang_python.functions import Functions, Types
from lang_python.edges import Uses
from lang_python.visibility import Reach
from lang_python.sdk.wire import (
    Edge,
    EdgeKind,
    Fragment,
    FunctionSummary,
    Module,
    Node,
    Resolution,
    Span,
    Symbol,
    TestCase,
)


@dataclass
class Indexed:
    """What indexing a file gave: its fragment and how many of its imports and names could not be resolved."""

    fragment: Fragment
    unresolved: int
    guessed: bool


class Unit:
    """Builds the fragment of one parsed file of the project."""

    def __init__(self, model: Model, reach: Reach, types: Types, parsed: ParsedFile):
        self.model = model
        self.reach = reach
        self.types = types
        self.parsed = parsed
        self.decls = parsed.decls
        self.rel = parsed.rel
        self.test = is_test_file(parsed.rel)
        self.fence = SNIPPETS_DIR in parsed.rel.split("/")
        self.resolver: Resolver = resolver_of(model, parsed)
        self.module_scope = parsed.scopes().module(parsed.tree)
        self.edges: list[Edge] = []
        self.seen: set[str] = set()
        self.targets: dict[str, dict[str, None]] = {}
        self.uses: Uses | None = None
        self.functions = Functions(model, parsed, self.resolver, types, self.module_scope, self.test)

    def edge(self, kind: EdgeKind, source: Node, to: str, site: Span | None, resolution: Resolution) -> None:
        owner = source.get("symbol") or source.get("module") or ""
        key = json.dumps([kind, owner, to, site])
        if key in self.seen:
            return
        self.seen.add(key)
        edge: Edge = {"kind": kind, "from": source, "to": to, "resolution": resolution}
        if site is not None:
            edge["site"] = site
        self.edges.append(edge)
        if kind in ("calls", "references") and "symbol" in source:
            self.targets.setdefault(source["symbol"], {})[to] = None

    def index(self) -> Indexed:
        decls = self.decls
        symbols = [self.symbol(decl) for decl in decls.list]
        for decl in decls.list:
            container: Node = {"symbol": decl.owner.id} if decl.owner is not None else {"module": decls.module}
            self.edge("contains", container, decl.id, None, "semantic")
        self.uses = Uses(self.model, self.parsed, self, self.fence)
        self.uses.run()
        functions: list[FunctionSummary] = []
        tests: list[TestCase] = []
        for decl in decls.list:
            if decl.body is None:
                continue
            functions.append(self.functions.summary(decl, self.function_scope(decl)))
            if decl.kind == "test":
                tests.append(test_case(decl, self.parsed.tree, self.targets.get(decl.id, {})))
        starts = {}
        for symbol in symbols:
            starts.setdefault(symbol["span"]["start"]["line"], symbol["id"])
        fragment: Fragment = {
            "file": {
                "path": self.rel,
                **({"generated": True} if is_generated(self.parsed.tree, self.parsed.tokens) else {}),
            },
            "modules": [self.module_record(decls.module)],
            "symbols": symbols,
            "edges": self.edges,
            "functions": functions,
            "tests": tests,
            "comments": self.parsed.tokens.grouped(starts),
        }
        return Indexed(fragment, self.uses.unresolved_imports, self.uses.guessed)

    def module_record(self, module: str) -> Module:
        if not self.test:
            return {"path": module}
        named = tested_module(module)
        known = self.model.project.modules()
        return {"path": module, "test_of": named if named is not None and named in known else directory_of(module)}

    # symbols

    def symbol(self, decl: Decl) -> Symbol:
        lines = self.parsed.lines
        symbol: Symbol = {
            "id": decl.id,
            "kind": decl.kind,
            "visibility": self.reach.visibility(decl, self.rel),
            "file": self.rel,
            "span": span_for(decl, lines) if decl.name != MODULE_BODY else span_of(decl.node),
            "name": decl.name,
        }
        if decl.owner is not None:
            symbol["owner"] = decl.owner.id
        if decl.name != MODULE_BODY:
            extent = extent_of(decl, self.parsed.tokens, lines)
            if extent is not None:
                symbol["extent"] = extent
            doc = doc_of(decl)
            if doc is not None:
                symbol["doc"] = doc
        role = role_of(decl, self.test)
        if role is not None:
            symbol["role"] = role
        if decl.kind == "field":
            field = self.types.field(decl, self.resolver, self.module_scope)
            if field is not None:
                symbol["type_ref"] = field[0]
                if field[1]:
                    symbol["optional"] = True
        return symbol

    # summaries

    def function_scope(self, decl: Decl) -> Scope:
        scope = self.uses.scopes.get(id(decl.body)) if self.uses is not None else None
        return scope if scope is not None else self.module_scope
