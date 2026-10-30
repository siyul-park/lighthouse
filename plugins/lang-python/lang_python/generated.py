"""Generated-code markers: a comment above the first statement says the file is not written by hand."""

import ast

from lang_python.comments import Tokens

GENERATED_MARKER = "@generated"


def is_generated(tree: ast.Module, tokens: Tokens) -> bool:
    """Whether a comment above the first statement marks the file as generated."""
    first = tree.body[0].lineno if tree.body else float("inf")
    for comment in tokens.comments:
        if comment.line >= first:
            break
        if GENERATED_MARKER in comment.text or "DO NOT EDIT" in comment.text.upper():
            return True
    return False
