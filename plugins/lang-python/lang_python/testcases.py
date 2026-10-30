"""Test cases: how a test is told a table, what checks on behalf of tests, and hand-written checks."""

import ast

from lang_python.decls import FUNCTION_NODES, dotted, has_decorator, is_docstring

ASSERTING = ("assert", "raises", "warns", "fail", "approx_raises")


def is_parametrized(node: ast.AST) -> bool:
    return has_decorator(node, "parametrize")


def is_row(element: ast.expr) -> bool:
    return isinstance(element, (ast.Tuple, ast.List, ast.Dict))


def literal_rows(value: ast.expr, bound: dict[str, ast.expr]) -> bool:
    """Whether an expression is a non-empty literal list of rows, written out or named at module level."""
    if isinstance(value, ast.Name) and value.id in bound:
        value = bound[value.id]
    return isinstance(value, (ast.List, ast.Tuple)) and len(value.elts) > 0 and all(is_row(e) for e in value.elts)


def module_literals(tree: ast.Module) -> dict[str, ast.expr]:
    found: dict[str, ast.expr] = {}
    for stmt in tree.body:
        if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
            found[stmt.targets[0].id] = stmt.value
        elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name) and stmt.value is not None:
            found[stmt.target.id] = stmt.value
    return found


def is_table(node: ast.FunctionDef | ast.AsyncFunctionDef, tree: ast.Module) -> bool:
    """Whether a test runs over rows written in the file: `parametrize`, or a loop over a literal list of rows."""
    if is_parametrized(node):
        return True
    bound = module_literals(tree)
    for child in ast.walk(node):
        if isinstance(child, (ast.For, ast.AsyncFor)) and literal_rows(child.iter, bound):
            return True
        if isinstance(child, ast.comprehension) and literal_rows(child.iter, bound):
            return True
    return False


def calls_assertion(node: ast.AST | None) -> bool:
    """Whether a function asserts, or calls an `assert*`, `raises` or `fail` function: it checks on behalf of tests."""
    if not isinstance(node, FUNCTION_NODES):
        return False
    for child in ast.walk(node):
        if isinstance(child, ast.Assert):
            return True
        if isinstance(child, ast.Call):
            name = (dotted(child.func) or "").rsplit(".", 1)[-1]
            if any(name.startswith(prefix) for prefix in ASSERTING):
                return True
    return False


def comparing(expr: ast.expr) -> bool:
    """A comparison, a negation, or a boolean combination of them."""
    if isinstance(expr, ast.UnaryOp):
        return isinstance(expr.op, ast.Not)
    if isinstance(expr, ast.BoolOp):
        return all(comparing(v) for v in expr.values)
    return isinstance(expr, ast.Compare)


def fails_only(body: list[ast.stmt]) -> bool:
    """A body of one statement that fails the test: a `raise`, or a call of `fail`."""
    if len(body) != 1:
        return False
    stmt = body[0]
    if isinstance(stmt, ast.Raise):
        return True
    if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call):
        return (dotted(stmt.value.func) or "").rsplit(".", 1)[-1] == "fail"
    return False


def manual_assertions(node: ast.AST) -> int:
    """The checks written out by hand: an `if` with no `else` that compares or negates and only fails."""
    count = 0
    for child in ast.walk(node):
        if isinstance(child, ast.If) and not child.orelse and comparing(child.test) and fails_only(child.body):
            count += 1
    return count


def body_statements(node: ast.AST) -> list[ast.stmt]:
    """The statements of a def's body without its docstring."""
    body = node.body  # type: ignore[attr-defined]
    return body[1:] if body and is_docstring(body[0]) else body
