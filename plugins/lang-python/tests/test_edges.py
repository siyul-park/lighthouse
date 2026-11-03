import unittest

from lang_python.edges import Uses
from tests.support import model_of

FILES = {
    "lib/__init__.py": "",
    "lib/base.py": "class Base:\n    def run(self): ...\n\n\nclass Contract:\n    pass\n",
    "lib/util.py": "def helper(): ...\n\n\nCONST = 1\n",
    "lib/app.py": (
        "from lib.base import Base\n"
        "from lib import util\n"
        "import os\n"
        "from .missing import gone\n\n\n"
        "class Child(Base):\n"
        "    def run(self):\n"
        "        util.helper()\n"
        "        return os.getcwd(), util.CONST\n\n\n"
        "def use(child: Child, other):\n"
        "    child.run()\n"
        "    other.run()\n"
        "    return getattr(other, 'run')\n"
    ),
}


class Sink:
    def __init__(self):
        self.edges: list[tuple[str, str, str, str]] = []
        self.loads: list[tuple[str, str, str]] = []

    def edge(self, kind, source, to, site, resolution):
        self.edges.append((kind, source.get("symbol") or source["module"], to, resolution))

    def load_edge(self, kind, source, to, site, resolution):
        self.edge(kind, source, to, site, resolution)
        self.loads.append((kind, source.get("symbol") or source["module"], to))


def uses(rel: str = "lib/app.py", fence: bool = False):
    model = model_of(FILES)
    sink = Sink()
    walker = Uses(model, model.file(rel), sink, fence)
    walker.run()
    return sink.edges, walker


class UsesTests(unittest.TestCase):
    def test_uses_run(self):
        edges, _ = uses()
        self.assertIn(("imports", "lib/app", "lib/base", "semantic"), edges)
        self.assertIn(("imports", "lib/app", "os", "semantic"), edges)
        self.assertIn(("calls", "lib/app::Child::run#method", "lib/util::helper#function", "semantic"), edges)
        self.assertIn(("references", "lib/app::Child::run#method", "lib/util::CONST#const", "semantic"), edges)
        self.assertIn(("calls", "lib/app::use#function", "lib/app::Child::run#method", "semantic"), edges)
        self.assertIn(("references", "lib/app::Child#type", "lib/base::Base#type", "semantic"), edges)

    def test_uses_run_guesses_members_of_untyped_receivers_by_name(self):
        edges, walker = uses()
        guessed = [e for e in edges if e[1] == "lib/app::use#function" and e[3] == "syntactic" and e[0] == "calls"]
        self.assertEqual(sorted({e[2] for e in guessed}), ["lib/app::Child::run#method", "lib/base::Base::run#method"])
        self.assertTrue(walker.guessed)

    def test_uses_run_counts_relative_imports_that_resolve_nowhere(self):
        edges, walker = uses()
        self.assertEqual(walker.unresolved_imports, 1)
        self.assertIn(("imports", "lib/app", "lib/missing", "syntactic"), edges)

    def test_uses_run_states_implements_for_contracts(self):
        model = model_of(
            {"m.py": "from abc import ABC\n\n\nclass Base(ABC):\n    pass\n\n\nclass Impl(Base):\n    pass\n"}
        )
        sink = Sink()
        Uses(model, model.file("m.py"), sink).run()
        self.assertIn(("implements", "m::Impl#type", "m::Base#type", "semantic"), sink.edges)

    def test_uses_run_reports_a_modules_exports_as_references(self):
        model = model_of({"m.py": "__all__ = ['f']\n\n\ndef f(): ...\n"})
        sink = Sink()
        Uses(model, model.file("m.py"), sink).run()
        self.assertIn(("references", "m", "m::f#function", "semantic"), sink.edges)

    def test_getattr_with_a_literal_prefix_uses_the_members_of_the_receivers_class(self):
        source = (
            "class Validator:\n"
            "    def check(self, kind):\n"
            "        return getattr(self, f'_check_{kind}')()\n\n"
            "    def _check_name(self): ...\n\n"
            "    def _check_size(self): ...\n\n"
            "    def _other(self): ...\n\n\n"
            "def loose(thing, kind):\n"
            "    return getattr(thing, '_check_' + kind)\n"
        )
        model = model_of({"m.py": source})
        sink = Sink()
        Uses(model, model.file("m.py"), sink).run()
        used = {to for kind, frm, to, res in sink.edges if kind == "references" and res == "syntactic"}
        self.assertEqual(used, {"m::Validator::_check_name#method", "m::Validator::_check_size#method"})

    def test_what_runs_as_the_module_loads_is_marked_load(self):
        source = (
            "from __future__ import annotations\n\n\n"
            "LIMIT = 4\nOTHER = LIMIT * 2\n\n\n"
            "class Base: ...\n\n\n"
            "def register(n): ...\n\n\n"
            "@register(LIMIT)\n"
            "def make(x=LIMIT, y: Base = None):\n"
            "    return x * OTHER\n\n\n"
            "class Row(Base):\n"
            "    size = LIMIT\n"
            "    def m(self, z=OTHER):\n"
            "        return LIMIT\n"
        )
        model = model_of({"m.py": source})
        sink = Sink()
        Uses(model, model.file("m.py"), sink).run()
        loads = {(owner.split("::")[-1], to.split("::")[-1]) for _, owner, to in sink.loads}
        self.assertIn(("OTHER#const", "LIMIT#const"), loads)
        self.assertIn(("make#function", "LIMIT#const"), loads)
        self.assertIn(("make#function", "register#function"), loads)
        self.assertIn(("Row#type", "Base#type"), loads)
        self.assertIn(("size#field", "LIMIT#const"), loads)
        self.assertIn(("m#method", "OTHER#const"), loads)
        # The body of a function and an annotation of a file that defers them run later.
        self.assertNotIn(("make#function", "OTHER#const"), loads)
        self.assertNotIn(("m#method", "LIMIT#const"), loads)
        self.assertNotIn(("make#function", "Base#type"), loads)

    def test_a_decorator_that_reads_a_later_constant_is_a_use_while_loading_only_at_the_top_level(self):
        source = "def outer():\n    @deco(K)\n    def inner(): ...\n    return inner\n\n\nK = 1\n\n\ndef deco(k): ...\n"
        model = model_of({"m.py": source})
        sink = Sink()
        Uses(model, model.file("m.py"), sink).run()
        self.assertEqual(sink.loads, [])


if __name__ == "__main__":
    unittest.main()
