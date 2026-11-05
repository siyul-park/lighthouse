"""What a file shows the others: its declarations without the bodies of its functions and methods."""

import ast
import copy
import hashlib

from lang_python.symbols import walk_flat


class _Bare(ast.NodeTransformer):
    """Empties the body of every function the module declares, and of every method but for the statements that set
    a field (`self.name = value`): a field a method declares is a member of its class, and the value gives its type.
    A function keeps its decorators, parameters and annotations."""

    def __init__(self) -> None:
        self._in_class = False

    def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.AST:
        node.body = self._fields(node) if self._in_class else [ast.Pass()]
        return node

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> ast.AST:
        node.body = self._fields(node) if self._in_class else [ast.Pass()]
        return node

    def visit_Module(self, node: ast.Module) -> ast.AST:
        node.body = _undocumented(node.body)
        return self.generic_visit(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> ast.AST:
        node.body = _undocumented(node.body)
        outer, self._in_class = self._in_class, True
        try:
            return self.generic_visit(node)
        finally:
            self._in_class = outer

    @staticmethod
    def _fields(node: ast.FunctionDef | ast.AsyncFunctionDef) -> list[ast.stmt]:
        """The statements of a method that set an attribute of its first parameter, in order."""
        first = node.args.args[0].arg if node.args.args else None
        kept = [stmt for stmt in walk_flat(node.body) if first is not None and _sets(stmt, first)]
        return kept or [ast.Pass()]


def _undocumented(body: list[ast.stmt]) -> list[ast.stmt]:
    """The statements without the docstring that leads them: it is a fact of the fragment, not of the interface."""
    first = body[0] if body else None
    documented = (
        isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant) and isinstance(first.value.value, str)
    )
    return body[1:] or [ast.Pass()] if documented else body


def _sets(stmt: ast.stmt, receiver: str) -> bool:
    """Whether a statement assigns `receiver.name`."""
    targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target] if isinstance(stmt, ast.AnnAssign) else []
    return any(
        isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name) and target.value.id == receiver
        for target in targets
    )


def digest(tree: ast.Module | None, text: str) -> str:
    """The hash of what a file shows the others: its tree without function bodies, else its text."""
    if tree is None:
        data = text
    else:
        data = ast.dump(_Bare().visit(copy.deepcopy(tree)), include_attributes=False)
    return hashlib.sha256(data.encode("utf-8", "surrogatepass")).hexdigest()
