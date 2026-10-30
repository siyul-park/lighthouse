import unittest

from lang_python.modules import directory_of, is_private_module, module_path, tested_module


class ModulesTests(unittest.TestCase):
    def test_module_path(self):
        self.assertEqual(module_path("pkg/mod.py"), "pkg/mod")
        self.assertEqual(module_path("pkg/__init__.py"), "pkg")
        self.assertEqual(module_path("__init__.py"), ".")
        self.assertEqual(module_path("pkg/mod.pyi"), "pkg/mod")

    def test_tested_module(self):
        known = frozenset({"a/b", "a/c", "x/shape", "y/shape", "tests/test_b", "a/tests/test_c"})
        self.assertEqual(tested_module("a/test_b", known), "a/b")
        self.assertEqual(tested_module("a/c_test", known), "a/c")
        self.assertIsNone(tested_module("a/other", known))
        self.assertIsNone(tested_module("a/test_missing", known))

    def test_tested_module_of_a_tests_directory_mirrors_the_layout_beside_it(self):
        known = frozenset({"pkg/core", "pkg/sub/core", "x/shape", "y/shape", "tests/test_core"})
        self.assertEqual(tested_module("pkg/tests/test_core", known), "pkg/core")
        self.assertEqual(tested_module("tests/sub/test_core", known), "pkg/sub/core")
        # `shape` is in two packages, so the name alone does not say which one.
        self.assertIsNone(tested_module("tests/test_shape", known))
        self.assertEqual(tested_module("x/tests/test_shape", known), "x/shape")
        self.assertIsNone(tested_module("tests/test_core", known | {"other/core"}))

    def test_directory_of(self):
        self.assertEqual(directory_of("pkg/tests/test_a"), "pkg")
        self.assertEqual(directory_of("tests/test_a"), ".")
        self.assertEqual(directory_of("pkg/test_a"), "pkg")
        self.assertEqual(directory_of("test_a"), ".")

    def test_is_private_module(self):
        self.assertTrue(is_private_module("pkg/_impl/mod"))
        self.assertTrue(is_private_module("_vendor"))
        self.assertFalse(is_private_module("pkg/__main__"))
        self.assertFalse(is_private_module("pkg/mod"))


if __name__ == "__main__":
    unittest.main()
