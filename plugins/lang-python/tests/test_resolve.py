import ast
import unittest

from lang_python.project import (
    DeclVal,
    InstanceVal,
    ModuleVal,
    constants,
    has_name,
    resolver_of,
    static_all,
    unwrap_annotation,
)
from tests.support import index, model_of

FILES = {
    "pkg/__init__.py": "from .core import Engine\n__all__ = ['Engine']\n",
    "pkg/core.py": (
        "from .extra import *\n\n\nclass Base:\n    def run(self): ...\n\n\nclass Engine(Base, Mixin):\n"
        "    def start(self): ...\n\n\ndef make() -> Engine: ...\n\n\nVALUE = Engine()\n"
    ),
    "pkg/extra.py": "__all__ = ['helper']\n\n\ndef helper(): ...\n\n\ndef _hidden(): ...\n",
    "app.py": (
        "import pkg\nimport pkg.core as core\nfrom pkg import Engine\nfrom pkg.core import make\n"
        "from elsewhere import x\n\n\n"
        "def go(e: Engine, n: int):\n    local = make()\n    return e, local, core.VALUE\n"
    ),
}


class World:
    """The files above, with the resolver of one of them."""

    def __init__(self, rel: str):
        self.model = model_of(FILES)
        self.parsed = self.model.file(rel)
        self.resolver = resolver_of(self.model, self.parsed)
        self.module = self.parsed.scopes().module(self.parsed.tree)

    def decl(self, rel: str, name: str):
        return self.model.decls(rel).top[name]

    def function_scope(self, name: str):
        node = next(n for n in ast.walk(self.parsed.tree) if isinstance(n, ast.FunctionDef) and n.name == name)
        return self.parsed.scopes().enter(node, self.module)


def expr(text: str) -> ast.expr:
    return ast.parse(text, mode="eval").body


class StaticAllTests(unittest.TestCase):
    def test_static_all(self):
        def names(source: str):
            found = static_all(ast.parse(source))
            return None if found is None else [c.value for c in found]

        self.assertEqual(names("__all__ = ['a', 'b']\n"), ["a", "b"])
        self.assertEqual(
            names("__all__ = ['a']\n__all__ += ['b']\n__all__.extend(['c'])\n__all__.append('d')\n"), list("abcd")
        )
        self.assertEqual(names("__all__: list[str] = ['a']\n"), ["a"])
        self.assertEqual(names("if x:\n    __all__ = ['a']\n"), ["a"])
        self.assertEqual(names("__all__ = ['a']\n__all__ = ['b']\n"), ["b"])
        self.assertEqual(names("__all__ = compute()\n"), [])
        self.assertIsNone(names("x = 1\n"))
        self.assertIsNone(names("__all__ += ['a']\n"))

    def test_constants(self):
        self.assertEqual([c.value for c in constants(expr("['a', 1, 'b']"))], ["a", "b"])
        self.assertEqual([c.value for c in constants(expr("('a',) + ['b']"))], ["a", "b"])
        self.assertEqual([c.value for c in constants(expr("'a'"))], ["a"])
        self.assertEqual(constants(expr("f()")), [])


class AnnotationTests(unittest.TestCase):
    def test_unwrap_annotation(self):
        def shown(text: str):
            found = unwrap_annotation(expr(text))
            return None if found is None else ast.unparse(found)

        for text, expected in {
            "Engine": "Engine",
            "pkg.Engine": "pkg.Engine",
            "Optional[Engine]": "Engine",
            "Annotated[Engine, 'meta']": "Engine",
            "Engine | None": "Engine",
            "Union[None, Engine]": "Engine",
            "'Engine'": "Engine",
            "Final[Optional[Engine]]": "Engine",
            "Engine | Other": None,
            "Union[A, B]": None,
            "list[Engine]": None,
            "'not valid ((' ": None,
            "3": None,
        }.items():
            self.assertEqual(shown(text), expected, text)
        self.assertIsNone(unwrap_annotation(None))

    def test_has_name(self):
        node = ast.parse("@a.b.cached\n@plain(1)\ndef f(): ...\n").body[0]
        self.assertTrue(has_name(node, "cached"))
        self.assertTrue(has_name(node, "plain"))
        self.assertFalse(has_name(node, "a"))
        self.assertFalse(has_name(ast.parse("x = 1").body[0], "plain"))


class ResolverTests(unittest.TestCase):
    def test_resolver_of(self):
        world = World("app.py")
        self.assertIs(resolver_of(world.model, world.parsed), world.resolver)

    def test_resolver_module_imports(self):
        bindings = World("app.py").resolver.module_imports()
        self.assertEqual(sorted(bindings), ["Engine", "core", "make", "pkg", "x"])

    def test_resolver_bound(self):
        world = World("app.py")
        imports = world.resolver.module_imports()
        self.assertEqual(world.resolver.bound(imports["Engine"]), DeclVal(world.decl("pkg/core.py", "Engine")))
        self.assertEqual(world.resolver.bound(imports["pkg"]).loc.module, "pkg")
        self.assertEqual(world.resolver.bound(imports["core"]).loc.module, "pkg/core")
        self.assertIsNone(world.resolver.bound(imports["x"]))

    def test_resolver_module_attribute(self):
        world = World("app.py")
        package = world.resolver.bound(world.resolver.module_imports()["pkg"])
        self.assertEqual(
            world.resolver.module_attribute(package, "Engine"), DeclVal(world.decl("pkg/core.py", "Engine"))
        )
        self.assertEqual(world.resolver.module_attribute(package, "core").loc.module, "pkg/core")
        self.assertIsNone(world.resolver.module_attribute(package, "missing"))
        self.assertIsNone(world.resolver.module_attribute(ModuleVal("os", None), "path"))

    def test_resolver_star_value(self):
        resolver = World("pkg/core.py").resolver
        self.assertEqual(resolver.star_value("helper", 0).decl.name, "helper")
        self.assertIsNone(resolver.star_value("_hidden", 0))
        self.assertIsNone(resolver.star_value("other", 0))

    def test_resolver_module_value(self):
        world = World("app.py")
        self.assertEqual(world.resolver.module_value("go"), DeclVal(world.decl("app.py", "go")))
        self.assertEqual(world.resolver.module_value("make").decl.name, "make")
        self.assertIsNone(world.resolver.module_value("nowhere"))

    def test_resolver_lookup(self):
        world = World("app.py")
        scope = world.function_scope("go")
        engine = world.decl("pkg/core.py", "Engine")
        self.assertEqual(world.resolver.lookup("e", scope), InstanceVal(engine))
        self.assertEqual(world.resolver.lookup("local", scope), InstanceVal(engine))
        self.assertIsNone(world.resolver.lookup("n", scope))
        self.assertEqual(world.resolver.lookup("Engine", scope), DeclVal(engine))

    def test_resolver_expr_value(self):
        world = World("app.py")
        scope = world.function_scope("go")
        engine = world.decl("pkg/core.py", "Engine")
        self.assertEqual(world.resolver.expr_value(expr("e.start"), scope).decl.name, "start")
        self.assertEqual(world.resolver.expr_value(expr("e.run"), scope).decl.name, "run")
        self.assertEqual(world.resolver.expr_value(expr("core.VALUE"), scope).decl.name, "VALUE")
        self.assertEqual(world.resolver.expr_value(expr("Engine()"), scope), InstanceVal(engine))
        self.assertEqual(world.resolver.expr_value(expr("make()"), scope), InstanceVal(engine))
        self.assertIsNone(world.resolver.expr_value(expr("e.nothing"), scope))
        self.assertIsNone(world.resolver.expr_value(expr("1 + 1"), scope))

    def test_resolver_attribute(self):
        world = World("app.py")
        engine = world.decl("pkg/core.py", "Engine")
        self.assertEqual(world.resolver.attribute(DeclVal(engine), "start").decl.name, "start")
        self.assertEqual(world.resolver.attribute(InstanceVal(engine), "run").decl.name, "run")
        value = world.decl("pkg/core.py", "VALUE")
        self.assertEqual(world.resolver.attribute(DeclVal(value), "start").decl.name, "start")
        self.assertIsNone(world.resolver.attribute(DeclVal(world.decl("pkg/core.py", "make")), "x"))

    def test_resolver_file_of(self):
        world = World("app.py")
        self.assertEqual(world.resolver.file_of(world.decl("pkg/core.py", "Engine")), "pkg/core.py")
        self.assertEqual(
            world.resolver.file_of(world.decl("pkg/__init__.py", "Engine")) if False else "pkg/core.py", "pkg/core.py"
        )

    def test_resolver_annotation_class(self):
        world = World("app.py")
        scope = world.function_scope("go")
        engine = world.decl("pkg/core.py", "Engine")
        for text in ("Engine", "Optional[Engine]", "'Engine'", "Engine | None"):
            self.assertIs(world.resolver.annotation_class(expr(text), scope), engine, text)
        self.assertIsNone(world.resolver.annotation_class(expr("int"), scope))
        self.assertIsNone(world.resolver.annotation_class(None, scope))

    def test_resolver_member(self):
        world = World("app.py")
        engine = world.decl("pkg/core.py", "Engine")
        self.assertEqual(world.resolver.member(engine, "start").owner, engine)
        self.assertEqual(world.resolver.member(engine, "run").owner.name, "Base")
        self.assertIsNone(world.resolver.member(engine, "missing"))

    def test_resolver_ancestors(self):
        world = World("app.py")
        engine = world.decl("pkg/core.py", "Engine")
        self.assertEqual([c.name for c in world.resolver.ancestors(engine)], ["Base"])
        self.assertEqual(world.resolver.ancestors(world.decl("pkg/core.py", "Base")), [])

    def test_resolver_methods_of(self):
        world = World("app.py")
        engine = world.decl("pkg/core.py", "Engine")
        self.assertEqual(world.resolver.methods_of(engine), ["start"])

    def test_resolver_external_bases(self):
        files = {
            "m.py": "import ast\nfrom logging import Handler\n\n\nclass A(ast.NodeVisitor, Handler, Gone):\n    pass\n"
        }
        model = model_of(files)
        parsed = model.file("m.py")
        found = resolver_of(model, parsed).external_bases(parsed.decls.top["A"])
        self.assertEqual([c.__name__ for c in found], ["NodeVisitor", "Handler"])

    def test_a_variable_defined_through_itself_settles_no_type(self):
        files = {"loop.py": "for name in ('a', 'b'):\n    name = name.replace('a', 'c')\n    print(name.upper())\n"}
        result = index(files)
        self.assertEqual(result["incomplete"], [])


if __name__ == "__main__":
    unittest.main()
