"""Control flow of one function body, normalized to the protocol's kinds."""

import ast
from typing import Callable

from lang_python.symbols import is_docstring
from lang_python.sdk.wire import Flow

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
        """Walks a def's body, or a lambda's expression, and fills `events`, `deepest` and `count`."""
        if isinstance(body, list):
            self._block(body[1:] if body and is_docstring(body[0]) else body)
        else:
            self.count += 1
            self._expr(body)

    def _block(self, body: list[ast.stmt]) -> None:
        for stmt in body:
            self._statement(stmt)

    def _statement(self, node: ast.stmt) -> None:
        self.count += 1
        if isinstance(node, ast.If):
            self._if_statement(node, chained=False)
        elif isinstance(node, LOOPS):
            self._loop(node)
        elif isinstance(node, ast.Match):
            self._match(node)
        elif isinstance(node, TRY_NODES):
            self._try_statement(node)
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                self._expr(decorator)
            self._nested(lambda: self._block(node.body))
        elif isinstance(node, ast.ClassDef):
            self._nested(lambda: self._block(node.body))
        else:
            self._children(node)

    def _children(self, node: ast.AST) -> None:
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.stmt):
                self._statement(child)
            elif isinstance(child, ast.expr):
                self._expr(child)
            else:
                self._children(child)

    def _expr(self, node: ast.expr) -> None:
        if isinstance(node, ast.BoolOp):
            self._logic(node)
        elif isinstance(node, ast.IfExp):
            self._event("if")
            self._expr(node.test)
            self._nested(lambda: (self._expr(node.body), self._expr(node.orelse)))
        elif isinstance(node, ast.Lambda):
            self.count += 1
            self._nested(lambda: self._expr(node.body))
        else:
            if isinstance(node, ast.Call) and self.is_self_call(node):
                self._event("recursion")
            for child in ast.iter_child_nodes(node):
                if isinstance(child, ast.expr):
                    self._expr(child)
                else:
                    self._children(child)

    def _if_statement(self, node: ast.If, chained: bool) -> None:
        self._event("else-if" if chained else "if")
        self._expr(node.test)
        self._nested(lambda: self._block(node.body))
        other = node.orelse
        if not other:
            return
        if len(other) == 1 and isinstance(other[0], ast.If) and other[0].col_offset == node.col_offset:
            self._if_statement(other[0], chained=True)
        else:
            self._event("else")
            self._nested(lambda: self._block(other))

    def _loop(self, node: ast.For | ast.AsyncFor | ast.While) -> None:
        self._event("loop")
        if isinstance(node, ast.While):
            self._expr(node.test)
        else:
            self._expr(node.target)
            self._expr(node.iter)
        self._nested(lambda: self._block(node.body))
        self._block(node.orelse)

    def _try_statement(self, node: ast.Try) -> None:
        self._block(node.body)
        for handler in node.handlers:
            self._event("catch")
            if handler.type is not None:
                self._expr(handler.type)
            self._nested(lambda handler=handler: self._block(handler.body))
        self._block(node.orelse)
        self._block(node.finalbody)

    def _match(self, node: ast.Match) -> None:
        arms = len([c for c in node.cases if not _irrefutable(c)])
        returning = bool(node.cases) and all(len(c.body) == 1 and isinstance(c.body[0], ast.Return) for c in node.cases)
        flow: Flow = {"kind": "switch", "nesting": self.nesting, "arms": arms}
        if returning:
            flow["returning"] = True
        self.events.append(flow)
        self._expr(node.subject)

        def cases() -> None:
            for case in node.cases:
                self.count += 1
                if case.guard is not None:
                    self._expr(case.guard)
                self._block(case.body)

        self._nested(cases)

    def _logic(self, node: ast.BoolOp) -> None:
        """One event per run of like operators in a chain of `and` and `or`, looking through parentheses."""
        operators: list[type] = []
        leaves: list[ast.expr] = []
        _chain(node, operators, leaves)
        index = 0
        while index < len(operators):
            end = index
            while end < len(operators) and operators[end] is operators[index]:
                end += 1
            self.events.append({"kind": "logic", "nesting": self.nesting, "operators": end - index})
            index = end
        for leaf in leaves:
            self._expr(leaf)

    def _nested(self, run: Callable[[], object]) -> None:
        self.nesting += 1
        self.deepest = max(self.deepest, self.nesting)
        run()
        self.nesting -= 1

    def _event(self, kind: str) -> None:
        self.events.append({"kind": kind, "nesting": self.nesting})  # type: ignore[typeddict-item]


def _chain(node: ast.expr, operators: list[type], leaves: list[ast.expr]) -> None:
    if not isinstance(node, ast.BoolOp):
        leaves.append(node)
        return
    for index, value in enumerate(node.values):
        if index > 0:
            operators.append(type(node.op))
        _chain(value, operators, leaves)


def _irrefutable(case: ast.match_case) -> bool:
    """The default arm: a wildcard or a bare capture, with no guard."""
    pattern = case.pattern
    return case.guard is None and isinstance(pattern, ast.MatchAs) and pattern.pattern is None
