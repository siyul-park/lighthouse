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

    def edge(self, kind, source, to, site, resolution):
        self.edges.append((kind, source.get("symbol") or source["module"], to, resolution))


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


if __name__ == "__main__":
    unittest.main()
