"""The fragment of one source file: its symbols, edges, function summaries, tests and comments."""

import ast
import json
from dataclasses import dataclass

from lang_python.decls import FUNCTION_NODES, MODULE_BODY, Decl
from lang_python.extent import doc_of, extent_of, span_for, start_of
from lang_python.flow import FlowWalker
from lang_python.model import Model, ParsedFile
from lang_python.paths import SNIPPETS_DIR, directory_of, is_test_file, tested_module
from lang_python.positions import span_of
from lang_python.resolve import DeclVal, Resolver, has_name, resolver_of
from lang_python.scopes import Scope
from lang_python.testcases import body_statements, calls_assertion, is_table, manual_assertions
from lang_python.types import Types, own_parameters, returns
from lang_python.uses import Uses
from lang_python.visibility import Reach
from lang_python.wire import Edge, EdgeKind, Fragment, FunctionSummary, Module, Node, Resolution, Span, Symbol, TestCase

NO_OVERRIDE = frozenset({"__init__", "__new__", "__init_subclass__", "__class_getitem__"})


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
            functions.append(self.summary(decl))
            if decl.kind == "test":
                tests.append(self.test_case(decl))
        starts = {}
        for symbol in symbols:
            starts.setdefault(symbol["span"]["start"]["line"], symbol["id"])
        fragment: Fragment = {
            "file": {
                "path": self.rel,
                **({"generated": True} if self.parsed.tokens.generated(self.parsed.tree) else {}),
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
        role = self.role(decl)
        if role is not None:
            symbol["role"] = role
        if decl.kind == "field":
            field = self.types.field(decl, self.resolver, self.module_scope)
            if field is not None:
                symbol["type_ref"] = field[0]
                if field[1]:
                    symbol["optional"] = True
        return symbol

    def role(self, decl: Decl) -> str | None:
        """The role of a declaration of a test file: fixtures are data, helpers check on behalf of tests."""
        if not self.test or decl.owner is not None or decl.name == MODULE_BODY:
            return None
        if decl.kind == "function":
            return "test-helper" if calls_assertion(decl.body) else "fixture"
        if decl.kind in ("type", "interface") and decl.traits & {"testcase", "test-class"}:
            return None
        return "fixture" if decl.kind in ("type", "interface", "const", "var") else None

    # summaries

    def function_scope(self, decl: Decl) -> Scope:
        scope = self.uses.scopes.get(id(decl.body)) if self.uses is not None else None
        return scope if scope is not None else self.module_scope

    def summary(self, decl: Decl) -> FunctionSummary:
        node = decl.body
        scope = self.function_scope(decl)
        walker = FlowWalker(lambda call: self.calls_self(call, decl, scope))
        walker.run(node.body)  # type: ignore[union-attr]
        statements = [] if isinstance(node, ast.Lambda) else body_statements(node)
        params = own_parameters(node, decl)
        summary: FunctionSummary = {
            "symbol": decl.id,
            "max_nesting": walker.deepest,
            "statements": walker.count,
            "top_level": 1 if isinstance(node, ast.Lambda) else len(statements),
            "params": len(params),
            "returns": returns(node),
            "tokens": self.token_count(node, statements),
            "flow": walker.events,
        }
        forwards = self.forward_target(decl, node, scope)
        if forwards is not None:
            summary["forwards_to"] = forwards
        signature = self.types.signature(decl, self.resolver, self.module_scope)
        if signature is not None:
            summary["signature"] = signature
        if self.implementation(decl):
            summary["implementation"] = True
        if self.types.constructs(decl, self.resolver, self.module_scope):
            summary["constructs"] = True
        if self.test:
            manual = manual_assertions(node)
            if manual > 0:
                summary["manual_assertions"] = manual
        return summary

    def token_count(self, node: ast.AST, statements: list[ast.stmt]) -> int:
        tokens = self.parsed.tokens
        if isinstance(node, ast.Lambda):
            return tokens.count(
                (node.body.lineno, node.body.col_offset), (node.body.end_lineno, node.body.end_col_offset)
            )
        if not statements:
            return 0
        line, col = start_of(statements[0], self.parsed.lines)
        last = statements[-1]
        return tokens.count((line, col), (last.end_lineno, last.end_col_offset))

    def calls_self(self, call: ast.Call, decl: Decl, scope: Scope) -> bool:
        value = self.resolver.expr_value(call.func, scope)
        return isinstance(value, DeclVal) and value.decl is decl

    def implementation(self, decl: Decl) -> bool:
        """Whether a method overrides one of a project base class, an interface or an abstract class, or says so."""
        node = decl.body
        if decl.kind != "method" or decl.owner is None or decl.name in NO_OVERRIDE or node is None:
            return False
        if has_name(node, "override"):
            return True
        for base in self.resolver.ancestors(decl.owner):
            target = self.model.decls(self.resolver.file_of(base))
            if target is not None and target.member(base, decl.name) is not None:
                return True
        return False

    def forward_target(self, decl: Decl, node: ast.AST, scope: Scope) -> str | None:
        """The call a body consists of when it passes every parameter on, in order: `return self.read(key)`."""
        if isinstance(node, ast.Lambda):
            return None
        statements = body_statements(node)
        if len(statements) != 1:
            return None
        only = statements[0]
        expr = only.value if isinstance(only, (ast.Return, ast.Expr)) else None
        while isinstance(expr, ast.Await):
            expr = expr.value
        if not isinstance(expr, ast.Call):
            return None
        params = own_parameters(node, decl)
        if not passes_on(node, params, expr):
            return None
        callee = expr.func
        method = decl.owner is not None
        everything = [*node.args.posonlyargs, *node.args.args]  # type: ignore[attr-defined]
        if isinstance(callee, ast.Name) and not method:
            pass
        elif (
            isinstance(callee, ast.Attribute)
            and method
            and everything
            and isinstance(callee.value, ast.Name)
            and callee.value.id == everything[0].arg
        ):
            pass
        else:
            return None
        value = self.resolver.expr_value(callee, scope)
        return value.decl.id if isinstance(value, DeclVal) else None

    def test_case(self, decl: Decl) -> TestCase:
        node = decl.body
        style = "table" if isinstance(node, FUNCTION_NODES) and is_table(node, self.parsed.tree) else "scenario"
        return {
            "symbol": decl.id,
            "nesting": 0,
            "style": style,
            "targets": list(self.targets.get(decl.id, {})),
        }


def passes_on(node: ast.AST, params: list[ast.arg], call: ast.Call) -> bool:
    """Whether a call's arguments are exactly the parameters, named and in order."""
    args = node.args  # type: ignore[attr-defined]
    positional = [*args.posonlyargs, *args.args]
    wanted = [p.arg for p in params if any(p is q for q in positional)]
    if args.vararg is not None:
        wanted.append("*" + args.vararg.arg)
    given: list[str] = []
    for argument in call.args:
        if isinstance(argument, ast.Starred) and isinstance(argument.value, ast.Name):
            given.append("*" + argument.value.id)
        elif isinstance(argument, ast.Name):
            given.append(argument.id)
        else:
            return False
    if given != wanted:
        return False
    keywords: dict[str, str] = {}
    for keyword in call.keywords:
        if not isinstance(keyword.value, ast.Name):
            return False
        keywords[keyword.arg or "**"] = keyword.value.id if keyword.arg is not None else "**" + keyword.value.id
    expected = {a.arg: a.arg for a in args.kwonlyargs}
    if args.kwarg is not None:
        expected["**"] = "**" + args.kwarg.arg
    return keywords == expected
