import ast
import unittest

from lang_python.project.scopes import Scope, ScopeTree, binding_scope

SOURCE = """\
import os
counter = 0


def outer(param):
    local = 1
    captured = 2

    def inner():
        nonlocal_free = captured
        return local, param, nonlocal_free, os, undefined

    def declares():
        global counter
        counter = 1

    return inner


class Box:
    attr = 1

    def method(self):
        return attr
"""


def scopes():
    tree = ast.parse(SOURCE)
    scopes = ScopeTree(SOURCE, "m.py")
    module = scopes.module(tree)
    defs = {n.name: n for n in ast.walk(tree) if isinstance(n, (ast.FunctionDef, ast.ClassDef))}
    outer = scopes.enter(defs["outer"], module)
    inner = scopes.enter(defs["inner"], outer)
    declares = scopes.enter(defs["declares"], outer)
    box = scopes.enter(defs["Box"], module)
    method = scopes.enter(defs["method"], box)
    return module, outer, inner, declares, box, method


class ScopeTreeTests(unittest.TestCase):
    def test_scope_tree_module(self):
        module = ScopeTree(SOURCE, "m.py").module(ast.parse(SOURCE))
        self.assertEqual((module.kind, module.parent), ("module", None))
        self.assertIsNotNone(module.table)

    def test_scope_tree_enter(self):
        module, outer, inner, _, box, method = scopes()
        self.assertEqual([s.kind for s in (outer, inner, box, method)], ["function", "function", "class", "function"])
        self.assertIs(inner.parent, outer)
        self.assertEqual(outer.table.get_name(), "outer")
        self.assertEqual(box.table.get_name(), "Box")

    def test_scope_tree_of_a_file_that_does_not_parse_has_no_tables(self):
        scopes = ScopeTree("def (:\n", "bad.py")
        self.assertIsNone(scopes.module(ast.parse("")).table)


class ScopeTests(unittest.TestCase):
    def test_scope_symbol(self):
        _, outer, _, _, _, _ = scopes()
        self.assertTrue(outer.symbol("local").is_local())
        self.assertIsNone(outer.symbol("nowhere"))
        self.assertIsNone(Scope("function", None, None).symbol("local"))


class BindingScopeTests(unittest.TestCase):
    def test_binding_scope(self):
        module, outer, inner, declares, _, method = scopes()
        self.assertIs(binding_scope(outer, "local"), outer)
        self.assertIs(binding_scope(inner, "local"), outer)
        self.assertIs(binding_scope(inner, "param"), outer)
        self.assertIs(binding_scope(inner, "nonlocal_free"), inner)

    def test_binding_scope_is_none_for_names_a_module_or_the_builtins_bind(self):
        module, outer, inner, declares, _, method = scopes()
        self.assertIsNone(binding_scope(module, "counter"))
        self.assertIsNone(binding_scope(inner, "os"))
        self.assertIsNone(binding_scope(inner, "undefined"))
        self.assertIsNone(binding_scope(declares, "counter"))

    def test_binding_scope_skips_class_scopes(self):
        _, _, _, _, _, method = scopes()
        self.assertIsNone(binding_scope(method, "attr"))


if __name__ == "__main__":
    unittest.main()
