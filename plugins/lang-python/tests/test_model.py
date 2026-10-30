import unittest

from tests.support import model_of

FILES = {
    "pkg/__init__.py": "",
    "pkg/a.py": "# note\nvalue = 1\n\n\nclass Thing:\n    pass\n",
    "pkg/b.py": "class Thing:\n    pass\n",
    "pkg/broken.py": "def (:\n",
}


class ParsedFileTests(unittest.TestCase):
    def test_parsed_file_tokens(self):
        parsed = model_of(FILES).file("pkg/a.py")
        self.assertEqual(parsed.tokens.comments[0].text, "# note")
        self.assertIs(parsed.tokens, parsed.tokens)

    def test_parsed_file_scopes(self):
        parsed = model_of(FILES).file("pkg/a.py")
        self.assertIsNotNone(parsed.scopes().module(parsed.tree).table)
        self.assertIs(parsed.scopes(), parsed.scopes())


class ModelTests(unittest.TestCase):
    def test_model_file(self):
        model = model_of(FILES)
        parsed = model.file("pkg/a.py")
        self.assertEqual((parsed.module, parsed.error), ("pkg/a", None))
        self.assertIs(model.file("pkg/a.py"), parsed)
        self.assertIsNone(model.file("pkg/missing.py"))

    def test_model_file_of_a_syntax_error_has_a_problem_and_no_tree(self):
        parsed = model_of(FILES).file("pkg/broken.py")
        self.assertIsNone(parsed.tree)
        self.assertTrue(parsed.error.startswith("1:"))

    def test_model_decls(self):
        model = model_of(FILES)
        self.assertEqual([d.name for d in model.decls("pkg/a.py").list], ["value", "Thing"])
        self.assertIsNone(model.decls("pkg/broken.py"))
        self.assertIsNone(model.decls("pkg/missing.py"))

    def test_model_named(self):
        model = model_of(FILES)
        self.assertEqual(sorted(d.module for d in model.named("Thing")), ["pkg/a", "pkg/b"])
        self.assertEqual(model.named("nothing"), [])

    def test_model_named_skips_the_stub_of_a_module_that_has_source(self):
        model = model_of({"m.py": "class A: ...\n", "m.pyi": "class A: ...\n", "only.pyi": "class B: ...\n"})
        self.assertEqual([d.module for d in model.named("A")], ["m"])
        self.assertEqual([d.module for d in model.named("B")], ["only"])


if __name__ == "__main__":
    unittest.main()
