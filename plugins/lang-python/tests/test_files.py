import tempfile
import unittest

from lang_python.project import Project, declared_roots, decode
from lang_python.project.files import is_ignored, is_stub


def project(files: dict[str, str]) -> Project:
    with tempfile.TemporaryDirectory() as root:
        return Project(root, files)


class FilePathTests(unittest.TestCase):
    def test_is_stub(self):
        self.assertTrue(is_stub("pkg/a.pyi"))
        self.assertFalse(is_stub("pkg/a.py"))

    def test_is_ignored(self):
        for rel in ("node_modules/x.py", "pkg/__pycache__/x.py", ".venv/x.py", "a/.hidden/b.py", "pkg/testdata/x.py"):
            self.assertTrue(is_ignored(rel), rel)
        for rel in ("pkg/a.py", ".hidden.py", "a/b/c.py"):
            self.assertFalse(is_ignored(rel), rel)

    def test_is_ignored_keeps_the_fences_of_hidden_hosts_visible(self):
        self.assertFalse(is_ignored("docs/.lighthouse-snippets/guide.md.1/snippet.py"))
        self.assertTrue(is_ignored("node_modules/.lighthouse-snippets/guide.md.1/snippet.py"))


class ProjectTests(unittest.TestCase):
    def test_project_read(self):
        found = project({"a.py": "x = 1\n"})
        self.assertEqual(found.read("a.py"), "x = 1\n")
        self.assertIsNone(found.read("missing.py"))

    def test_project_read_prefers_overlays_and_reads_snippets(self):
        with tempfile.TemporaryDirectory() as root:
            found = Project(root, {"a.py": "overlay\n"}, {"snip/snippet.py": "fence\n"})
            self.assertEqual(found.read("snip/snippet.py"), "fence\n")
            self.assertEqual(found.read("a.py"), "overlay\n")

    def test_project_modules(self):
        found = project({"pkg/__init__.py": "", "pkg/a.py": "", "pkg/a.pyi": "", "b.py": ""})
        self.assertEqual(found.modules(), {"pkg", "pkg/a", "b"})

    def test_project_find(self):
        found = project(
            {"pkg/__init__.py": "", "pkg/a.py": "", "ns/b.py": "", "src/lib/mod.py": "", "pyproject.toml": ""}
        )
        self.assertEqual((found.find("pkg.a").module, found.find("pkg.a").is_package), ("pkg/a", False))
        self.assertEqual((found.find("pkg").rel, found.find("pkg").is_package), ("pkg/__init__.py", True))
        namespace = found.find("ns")
        self.assertEqual((namespace.module, namespace.rel, namespace.is_package), ("ns", None, True))
        self.assertEqual(found.find("lib.mod").rel, "src/lib/mod.py")
        self.assertIsNone(found.find("nowhere"))

    def test_project_find_looks_beside_the_importing_script_first(self):
        found = project({"tool/util.py": "", "util.py": "", "tool/run.py": ""})
        self.assertEqual(found.find("util", "tool/run.py").rel, "tool/util.py")
        self.assertEqual(found.find("util").rel, "util.py")

    def test_project_find_relative(self):
        found = project({"pkg/__init__.py": "", "pkg/a.py": "", "pkg/sub/__init__.py": "", "pkg/sub/b.py": ""})
        self.assertEqual(found.find_relative("pkg/sub/b.py", 1, None).module, "pkg/sub")
        self.assertEqual(found.find_relative("pkg/sub/b.py", 2, "a").module, "pkg/a")
        self.assertIsNone(found.find_relative("pkg/sub/b.py", 1, "missing"))
        self.assertIsNone(found.find_relative("pkg/a.py", 4, "x"))

    def test_project_relative_path(self):
        found = project({})
        self.assertEqual(found.relative_path("pkg/sub/b.py", 2, "a.c"), "pkg/a/c")
        self.assertEqual(found.relative_path("pkg/sub/b.py", 1, None), "pkg/sub")
        self.assertEqual(found.relative_path("b.py", 1, None), ".")


class DeclaredRootsTests(unittest.TestCase):
    def test_declared_package_dirs_of_setuptools_and_hatch(self):
        data = {
            "tool": {
                "setuptools": {"package-dir": {"": "lib"}},
                "hatch": {"build": {"targets": {"wheel": {"packages": ["python/pkg"]}}}},
            }
        }
        self.assertEqual(declared_roots(data), ["lib", "python"])

    def test_declared_package_dirs_of_find_where_and_hatch_sources(self):
        data = {
            "tool": {
                "setuptools": {"packages": {"find": {"where": ["src", 3]}}},
                "hatch": {"build": {"sources": ["lib"], "packages": ["flat"]}},
            }
        }
        self.assertEqual(declared_roots(data), ["src", "lib"])
        self.assertEqual(declared_roots({"tool": {"hatch": {"build": "nonsense"}}}), [])
        self.assertEqual(declared_roots({}), [])

    def test_source_bytes_honour_coding_cookies_and_byte_order_marks(self):
        self.assertEqual(decode("﻿x = 1\r\n".encode("utf-8")), "x = 1\n")
        self.assertEqual(decode("# -*- coding: latin-1 -*-\nx = 'é'\n".encode("latin-1")).splitlines()[1], "x = 'é'")


if __name__ == "__main__":
    unittest.main()
