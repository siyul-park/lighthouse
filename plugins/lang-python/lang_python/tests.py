"""Test cases: how a test is told a table, what checks on behalf of tests, and hand-written checks."""

import ast

from lang_python.sdk.wire import TestCase
from lang_python.symbols import FUNCTION_NODES, MODULE_BODY, Decl, dotted, has_decorator, is_docstring

ASSERTING = ("assert", "raises", "warns", "fail", "approx_raises")


def is_test_file(rel: str) -> bool:
    """Whether a project-relative file is a test file by the conventions the manifest declares."""
    parts = rel.split("/")
    name = parts[-1]
    stem = name.rsplit(".", 1)[0]
    if stem == "conftest" or stem.startswith("test_") or stem.endswith("_test"):
        return True
    return "tests" in parts[:-1] or "test" in parts[:-1]


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


def role_of(decl: Decl, test: bool) -> str | None:
    """The role of a declaration of a test file: fixtures are data, helpers check on behalf of tests."""
    if not test or decl.owner is not None or decl.name == MODULE_BODY:
        return None
    if decl.kind == "function":
        return "test-helper" if calls_assertion(decl.body) else "fixture"
    if decl.kind in ("type", "interface") and decl.traits & {"testcase", "test-class"}:
        return None
    return "fixture" if decl.kind in ("type", "interface", "const", "var") else None


def test_case(decl: Decl, tree: ast.Module, targets: dict[str, None]) -> TestCase:
    """The test case of a `test` symbol: a table when it runs over rows written in the file."""
    node = decl.body
    table = isinstance(node, FUNCTION_NODES) and is_table(node, tree)
    return {"symbol": decl.id, "nesting": 0, "style": "table" if table else "scenario", "targets": list(targets)}
