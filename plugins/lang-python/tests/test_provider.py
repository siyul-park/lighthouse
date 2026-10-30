import unittest

from lang_python.provider import ALIASES, language

from tests.support import edges, fragment, index, symbols


class ManifestTests(unittest.TestCase):
    def test_the_manifest_declares_python_with_linguist_aliases(self):
        declared = language()
        self.assertEqual(declared["id"], "python")
        self.assertEqual(declared["globs"], ["**/*.{py,pyi}"])
        self.assertEqual(declared["aliases"], ["python", "py", "py3", "python3", "rusthon"])
        self.assertEqual(ALIASES, declared["aliases"])
        self.assertEqual(declared["capabilities"], ["semantic-edges", "extent", "reference-sites", "overlays"])


class FactTests(unittest.TestCase):
    def test_symbols_have_owners_kinds_and_visibility(self):
        result = index(
            {
                "m.py": '"""Mod."""\n\n\nclass _Hidden:\n    def run(self):\n        pass\n\n\n'
                "class Shown:\n    x = 1\n\n"
                "    def _tuck(self):\n        pass\n\n\ndef top():\n    pass\n"
            }
        )
        found = symbols(result, "m.py")
        self.assertEqual(found["m::_Hidden#type"]["visibility"], "private")
        self.assertEqual(found["m::_Hidden::run#method"]["visibility"], "internal")
        self.assertEqual(found["m::Shown::x#field"]["owner"], "m::Shown#type")
        self.assertEqual(found["m::Shown::_tuck#method"]["visibility"], "private")
        self.assertEqual(found["m::top#function"]["visibility"], "public")

    def test_all_decides_what_is_public(self):
        result = index({"m.py": '__all__ = ["a"]\n\n\ndef a():\n    pass\n\n\ndef b():\n    pass\n'})
        found = symbols(result, "m.py")
        self.assertEqual(found["m::a#function"]["visibility"], "public")
        self.assertEqual(found["m::b#function"]["visibility"], "private")

    def test_a_package_reexport_lifts_an_underscored_name(self):
        result = index(
            {
                "pkg/__init__.py": 'from .impl import _x\n\n__all__ = ["_x"]\n',
                "pkg/impl.py": "def _x():\n    pass\n\n\ndef _y():\n    pass\n",
            }
        )
        found = symbols(result, "pkg/impl.py")
        self.assertEqual(found["pkg/impl::_x#function"]["visibility"], "public")
        self.assertEqual(found["pkg/impl::_y#function"]["visibility"], "private")

    def test_decorated_defs_extend_to_the_decorator_and_docstrings_are_docs(self):
        result = index(
            {"m.py": "import functools\n\n# note\n@functools.cache\ndef f():\n    '''Doc.\n\n    More.\n    '''\n"}
        )
        symbol = symbols(result, "m.py")["m::f#function"]
        self.assertEqual(symbol["extent"]["start"], {"line": 3, "col": 1})
        self.assertEqual(symbol["span"]["start"], {"line": 4, "col": 1})
        self.assertEqual(symbol["doc"], "Doc.\n\nMore.")

    def test_enum_members_are_variants_and_dataclass_fields_are_fields(self):
        result = index(
            {
                "m.py": "import enum\nfrom dataclasses import dataclass\n\n\nclass C(enum.Enum):\n    A = 1\n\n\n"
                "@dataclass\nclass D:\n    x: int\n    y: int = 0\n"
            }
        )
        found = symbols(result, "m.py")
        self.assertEqual(found["m::C::A#variant"]["kind"], "variant")
        self.assertEqual(found["m::D::x#field"]["kind"], "field")
        self.assertNotIn("optional", found["m::D::x#field"])
        self.assertTrue(found["m::D::y#field"]["optional"])

    def test_overloads_merge_into_one_symbol(self):
        source = (
            "from typing import overload\n\n\n@overload\ndef f(a: int) -> int: ...\n"
            "@overload\ndef f(a: str) -> str: ...\ndef f(a):\n    return a\n"
        )
        result = index({"m.py": source})
        self.assertEqual([s for s in symbols(result, "m.py") if s.startswith("m::f")], ["m::f#function"])
        self.assertEqual(len(fragment(result, "m.py")["functions"]), 1)

    def test_module_level_statements_are_a_bodyless_module_function(self):
        result = index({"m.py": "def f():\n    pass\n\n\nf()\n"})
        self.assertIn("m::<module>#function", symbols(result, "m.py"))
        self.assertIn(("calls", "m::<module>#function", "m::f#function", "semantic"), edges(result, "m.py"))
        names = [fn["symbol"] for fn in fragment(result, "m.py")["functions"]]
        self.assertNotIn("m::<module>#function", names)


class EdgeTests(unittest.TestCase):
    def test_edges_resolve_through_imports_annotations_and_self(self):
        result = index(
            {
                "pkg/__init__.py": "",
                "pkg/a.py": "class A:\n    def go(self):\n        return 1\n",
                "pkg/b.py": "from .a import A\n\n\nclass B:\n    def use(self, a: A):\n        a.go()\n"
                "        self.use(a)\n",
            }
        )
        found = edges(result, "pkg/b.py")
        self.assertIn(("imports", "pkg/b", "pkg/a", "semantic"), found)
        self.assertIn(("calls", "pkg/b::B::use#method", "pkg/a::A::go#method", "semantic"), found)
        self.assertIn(("calls", "pkg/b::B::use#method", "pkg/b::B::use#method", "semantic"), found)

    def test_an_untyped_receiver_is_syntactic_and_an_unknown_name_is_nothing(self):
        result = index(
            {
                "m.py": "class A:\n    def go(self):\n        pass\n\n\ndef f(x):\n    x.go()\n    x.nothing()\n",
            }
        )
        found = edges(result, "m.py", "calls")
        self.assertEqual(found, [("calls", "m::f#function", "m::A::go#method", "syntactic")])

    def test_a_local_that_shadows_a_module_name_is_not_an_edge(self):
        result = index({"m.py": "def g():\n    pass\n\n\ndef f():\n    g = 1\n    return g\n"})
        self.assertEqual(edges(result, "m.py", "references"), [])

    def test_implements_runs_to_project_interfaces_and_abstract_classes(self):
        result = index(
            {
                "m.py": "from abc import ABC\nfrom typing import Protocol\n\n\nclass P(Protocol):\n    pass\n\n\n"
                "class B(ABC):\n    pass\n\n\nclass C(P, B):\n    pass\n\n\nclass D(C):\n    pass\n"
            }
        )
        found = edges(result, "m.py")
        self.assertIn(("implements", "m::C#type", "m::P#interface", "semantic"), found)
        self.assertIn(("implements", "m::C#type", "m::B#type", "semantic"), found)
        self.assertIn(("references", "m::D#type", "m::C#type", "semantic"), found)

    def test_a_relative_import_that_resolves_nowhere_is_a_notice(self):
        result = index({"m.py": "from .ghost import x\n"})
        self.assertEqual(edges(result, "m.py", "imports"), [("imports", "m", "ghost", "syntactic")])
        self.assertTrue(any("relative import" in n for n in result["notices"]))

    def test_src_layout_and_namespace_packages_resolve(self):
        result = index(
            {
                "pyproject.toml": '[project]\nname = "p"\n',
                "src/p/__init__.py": "",
                "src/p/m.py": "def f():\n    pass\n",
                "ns/x/y.py": "def g():\n    pass\n",
                "use.py": "import p.m\nimport ns.x.y\n\n\ndef use():\n    p.m.f()\n    ns.x.y.g()\n",
            }
        )
        found = edges(result, "use.py")
        self.assertIn(("calls", "use::use#function", "src/p/m::f#function", "semantic"), found)
        self.assertIn(("calls", "use::use#function", "ns/x/y::g#function", "semantic"), found)


class SummaryTests(unittest.TestCase):
    def test_flow_and_counts(self):
        source = (
            "def f(a, b):\n    if a and b or not a:\n        return 1\n    elif b:\n        return 2\n    else:\n"
            "        for x in a:\n            pass\n    return f(a, b)\n"
        )
        summary = fragment(index({"m.py": source}), "m.py")["functions"][0]
        kinds = [e["kind"] for e in summary["flow"]]
        self.assertEqual(kinds, ["if", "logic", "logic", "else-if", "else", "loop", "recursion"])
        self.assertEqual(summary["params"], 2)
        self.assertEqual(summary["max_nesting"], 2)
        self.assertEqual(summary["top_level"], 2)

    def test_the_receiver_is_not_a_parameter_and_static_methods_have_none(self):
        source = "class A:\n    def m(self, x):\n        pass\n\n    @staticmethod\n    def s(x):\n        pass\n"
        found = {f["symbol"]: f for f in fragment(index({"m.py": source}), "m.py")["functions"]}
        self.assertEqual(found["m::A::m#method"]["params"], 1)
        self.assertEqual(found["m::A::s#method"]["params"], 1)

    def test_tokens_do_not_depend_on_the_interpreter_for_fstrings(self):
        source = 'def f(x):\n    return f"a{x}b{x!r}"\n'
        summary = fragment(index({"m.py": source}), "m.py")["functions"][0]
        self.assertEqual(summary["tokens"], 2)


class TestCaseTests(unittest.TestCase):
    def test_pytest_and_unittest_cases(self):
        source = (
            "import unittest\nimport pytest\n\nROWS = [(1, 2)]\n\n\ndef test_a():\n    assert 1\n\n\n"
            '@pytest.mark.parametrize("a", [1, 2])\ndef test_b(a):\n    assert a\n\n\nclass TestC:\n'
            "    def test_c(self):\n        for r in ROWS:\n            assert r\n\n\n"
            "class U(unittest.TestCase):\n    def test_u(self):\n        self.assertTrue(1)\n\n"
            "    def helper(self):\n        pass\n"
        )
        result = index({"test_x.py": source})
        found = symbols(result, "test_x.py")
        tests = {t["symbol"]: t["style"] for t in fragment(result, "test_x.py")["tests"]}
        self.assertEqual(
            tests,
            {
                "test_x::test_a#test": "scenario",
                "test_x::test_b#test": "table",
                "test_x::TestC::test_c#test": "table",
                "test_x::U::test_u#test": "scenario",
            },
        )
        self.assertEqual(found["test_x::U::helper#method"]["kind"], "method")
        self.assertEqual(fragment(result, "test_x.py")["modules"][0]["test_of"], ".")

    def test_a_non_test_file_declares_pytest_names_as_functions(self):
        result = index({"lib.py": "def test_a():\n    pass\n"})
        self.assertEqual(symbols(result, "lib.py")["lib::test_a#function"]["kind"], "function")


class FileTests(unittest.TestCase):
    def test_a_syntax_error_is_incomplete_and_the_file_keeps_a_fragment(self):
        result = index({"bad.py": "def f(:\n    pass\n", "ok.py": "def g():\n    pass\n"})
        self.assertEqual(len(result["incomplete"]), 1)
        self.assertEqual(result["incomplete"][0]["path"], "bad.py")
        self.assertTrue(result["incomplete"][0]["reason"].startswith("1:7: "))
        self.assertEqual(fragment(result, "bad.py")["symbols"], [])
        self.assertEqual(len(fragment(result, "ok.py")["symbols"]), 1)

    def test_stubs_hidden_directories_and_testdata_have_empty_fragments(self):
        files = {
            "a.pyi": "def f() -> int: ...\n",
            ".h/x.py": "def f():\n    pass\n",
            "testdata/y.py": "def f():\n    pass\n",
        }
        result = index(files)
        for path in files:
            self.assertEqual(fragment(result, path)["symbols"], [])
        self.assertEqual(result["incomplete"], [])

    def test_a_missing_file_is_incomplete(self):
        result = index({"a.py": ""}, paths=["gone.py"])
        self.assertEqual(result["incomplete"], [{"path": "gone.py", "reason": "cannot read the file"}])

    def test_unknown_options_are_refused(self):
        result = index({"a.py": ""}, options={"nope": 1})
        self.assertEqual(result["fragments"], [])
        self.assertIn("nope", result["incomplete"][0]["reason"])

    def test_generated_markers_before_the_first_statement(self):
        result = index({"a.py": "# @generated\nX = 1\n", "b.py": "X = 1\n# @generated\n"})
        self.assertTrue(fragment(result, "a.py")["file"]["generated"])
        self.assertNotIn("generated", fragment(result, "b.py")["file"])

    def test_columns_count_utf8_bytes(self):
        result = index({"a.py": 'def f():\n    return "é" + g()\n\n\ndef g():\n    pass\n'})
        site = next(e["site"] for e in fragment(result, "a.py")["edges"] if e["kind"] == "calls")
        self.assertEqual(site["start"], {"line": 2, "col": 19})


if __name__ == "__main__":
    unittest.main()
