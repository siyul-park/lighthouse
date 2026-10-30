"""Control flow of one function body, normalized to the protocol's kinds."""

import ast
from typing import Callable

from lang_python.decls import is_docstring
from lang_python.wire import Flow

SelfCall = Callable[[ast.Call], bool]
TRY_NODES = tuple(t for t in (ast.Try, getattr(ast, "TryStar", None)) if t is not None)
LOOPS = (ast.For, ast.AsyncFor, ast.While)


class FlowWalker:
    """Walks one function body: its flow events, the deepest nesting and the number of statements."""

    def __init__(self, is_self_call: SelfCall):
        self.is_self_call = is_self_call
        self.events: list[Flow] = []
        self.deepest = 0
        self.count = 0
        self.nesting = 0

    def run(self, body: list[ast.stmt] | ast.expr) -> None:
        if isinstance(body, list):
            self.block(body[1:] if body and is_docstring(body[0]) else body)
        else:
            self.count += 1
            self.expr(body)

    def block(self, body: list[ast.stmt]) -> None:
        for stmt in body:
            self.statement(stmt)

    def statement(self, node: ast.stmt) -> None:
        self.count += 1
        if isinstance(node, ast.If):
            self.if_statement(node, chained=False)
        elif isinstance(node, LOOPS):
            self.loop(node)
        elif isinstance(node, ast.Match):
            self.match(node)
        elif isinstance(node, TRY_NODES):
            self.try_statement(node)
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                self.expr(decorator)
            self.nested(lambda: self.block(node.body))
        elif isinstance(node, ast.ClassDef):
            self.nested(lambda: self.block(node.body))
        else:
            self.children(node)

    def children(self, node: ast.AST) -> None:
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.stmt):
                self.statement(child)
            elif isinstance(child, ast.expr):
                self.expr(child)
            else:
                self.children(child)

    def expr(self, node: ast.expr) -> None:
        if isinstance(node, ast.BoolOp):
            self.logic(node)
        elif isinstance(node, ast.IfExp):
            self.event("if")
            self.expr(node.test)
            self.nested(lambda: (self.expr(node.body), self.expr(node.orelse)))
        elif isinstance(node, ast.Lambda):
            self.count += 1
            self.nested(lambda: self.expr(node.body))
        else:
            if isinstance(node, ast.Call) and self.is_self_call(node):
                self.event("recursion")
            for child in ast.iter_child_nodes(node):
                if isinstance(child, ast.expr):
                    self.expr(child)
                else:
                    self.children(child)

    def if_statement(self, node: ast.If, chained: bool) -> None:
        self.event("else-if" if chained else "if")
        self.expr(node.test)
        self.nested(lambda: self.block(node.body))
        other = node.orelse
        if not other:
            return
        if len(other) == 1 and isinstance(other[0], ast.If) and other[0].col_offset == node.col_offset:
            self.if_statement(other[0], chained=True)
        else:
            self.event("else")
            self.nested(lambda: self.block(other))

    def loop(self, node: ast.For | ast.AsyncFor | ast.While) -> None:
        self.event("loop")
        if isinstance(node, ast.While):
            self.expr(node.test)
        else:
            self.expr(node.target)
            self.expr(node.iter)
        self.nested(lambda: self.block(node.body))
        self.block(node.orelse)

    def try_statement(self, node: ast.Try) -> None:
        self.block(node.body)
        for handler in node.handlers:
            self.event("catch")
            if handler.type is not None:
                self.expr(handler.type)
            self.nested(lambda handler=handler: self.block(handler.body))
        self.block(node.orelse)
        self.block(node.finalbody)

    def match(self, node: ast.Match) -> None:
        arms = len([c for c in node.cases if not irrefutable(c)])
        returning = bool(node.cases) and all(len(c.body) == 1 and isinstance(c.body[0], ast.Return) for c in node.cases)
        flow: Flow = {"kind": "switch", "nesting": self.nesting, "arms": arms}
        if returning:
            flow["returning"] = True
        self.events.append(flow)
        self.expr(node.subject)

        def cases() -> None:
            for case in node.cases:
                self.count += 1
                if case.guard is not None:
                    self.expr(case.guard)
                self.block(case.body)

        self.nested(cases)

    def logic(self, node: ast.BoolOp) -> None:
        """One event per run of like operators in a chain of `and` and `or`, looking through parentheses."""
        operators: list[type] = []
        leaves: list[ast.expr] = []
        chain(node, operators, leaves)
        index = 0
        while index < len(operators):
            end = index
            while end < len(operators) and operators[end] is operators[index]:
                end += 1
            self.events.append({"kind": "logic", "nesting": self.nesting, "operators": end - index})
            index = end
        for leaf in leaves:
            self.expr(leaf)

    def nested(self, run: Callable[[], object]) -> None:
        self.nesting += 1
        self.deepest = max(self.deepest, self.nesting)
        run()
        self.nesting -= 1

    def event(self, kind: str) -> None:
        self.events.append({"kind": kind, "nesting": self.nesting})  # type: ignore[typeddict-item]


def chain(node: ast.expr, operators: list[type], leaves: list[ast.expr]) -> None:
    if not isinstance(node, ast.BoolOp):
        leaves.append(node)
        return
    for index, value in enumerate(node.values):
        if index > 0:
            operators.append(type(node.op))
        chain(value, operators, leaves)


def irrefutable(case: ast.match_case) -> bool:
    """The default arm: a wildcard or a bare capture, with no guard."""
    pattern = case.pattern
    return case.guard is None and isinstance(pattern, ast.MatchAs) and pattern.pattern is None
