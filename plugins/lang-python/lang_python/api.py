"""What a file shows the others: its declarations without the bodies of its module functions."""

import ast
import copy
import hashlib


class _Bare(ast.NodeTransformer):
    """Empties the body of every function the module declares; a class keeps its methods, whose bodies set its
    fields, and a function keeps its decorators, parameters and annotations."""

    def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.AST:
        node.body = [ast.Pass()]
        return node

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> ast.AST:
        node.body = [ast.Pass()]
        return node

    def visit_ClassDef(self, node: ast.ClassDef) -> ast.AST:
        return node


def digest(tree: ast.Module | None, text: str) -> str:
    """The hash of what a file shows the others: its tree without function bodies, else its text."""
    if tree is None:
        data = text
    else:
        data = ast.dump(_Bare().visit(copy.deepcopy(tree)), include_attributes=False)
    return hashlib.sha256(data.encode("utf-8", "surrogatepass")).hexdigest()
