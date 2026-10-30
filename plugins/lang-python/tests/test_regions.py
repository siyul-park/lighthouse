import unittest
from typing import Any

from lang_python.regions import Remap, host_line, plan_snippets
from tests.support import edges, fragment, index

LIB = {"pkg/__init__.py": "", "pkg/core.py": "def compute(x):\n    return x\n"}


def host(path: str, text: str, code: str, start: int | None = None) -> dict[str, Any]:
    """A Markdown host whose one fence holds `code`; the fence text sits at `start` in the host."""
    at = text.index(code) if start is None else start
    source = {
        "id": "1",
        "rule_id": "markdown-code",
        "node_pointer": "/children/0",
        "source_pointer": "/children/0/literal",
        "provider": "python",
        "text": code,
        "source_range": [at, at + len(code.encode())],
    }
    return {"path": path, "hash": "0", "text": text, "sources": [source]}


def two_fences() -> dict[str, Any]:
    """A Markdown host with two fences, the second written first in the list of sources."""
    first, second = "a = 1\n", "def demo():\n    pass\n"
    text = "# T\n\n```python\n" + first + "```\n\n```python\n" + second + "```\n"
    one = host("docs/guide.md", text, first)
    two = host("docs/guide.md", text, second)
    one["sources"] = [two["sources"][0] | {"id": "2"}, one["sources"][0]]
    return one


class RegionTests(unittest.TestCase):
    def test_plan_snippets(self):
        snippets = plan_snippets([two_fences()])
        self.assertEqual(
            [(s.rel, s.text.text) for s in snippets],
            [
                ("docs/.lighthouse-snippets/guide.md.1/snippet.py", "a = 1\n"),
                ("docs/.lighthouse-snippets/guide.md.2/snippet.py", "def demo():\n    pass\n"),
            ],
        )
        self.assertEqual(snippets[0].text.host[0], (15, 16))
        self.assertEqual(plan_snippets([]), [])

    def test_host_line(self):
        first, second = plan_snippets([two_fences()])
        self.assertEqual((host_line(first), host_line(second)), (4, 8))

    def test_remap_fragment(self):
        snippet = plan_snippets([two_fences()])[1]
        where = {"start": {"line": 1, "col": 1}, "end": {"line": 2, "col": 9}}
        elsewhere = {"start": {"line": 40, "col": 1}, "end": {"line": 41, "col": 1}}
        base = {"kind": "function", "visibility": "public", "file": snippet.rel}
        source = {
            "file": {"path": snippet.rel},
            "modules": [{"path": "m"}],
            "symbols": [
                {**base, "id": "m::demo#function", "span": where},
                {**base, "id": "m::gone#function", "span": elsewhere, "owner": "m::demo#function"},
            ],
            "edges": [],
            "functions": [],
            "tests": [],
            "comments": [],
        }  # fmt: skip
        mapped = Remap(snippet, "m").fragment(source)
        self.assertEqual(mapped["file"], {"path": "docs/guide.md"})
        self.assertEqual([s["id"] for s in mapped["symbols"]], ["m::demo#function"])
        self.assertEqual(mapped["symbols"][0]["file"], "docs/guide.md")
        self.assertEqual(mapped["symbols"][0]["span"]["start"], {"line": 8, "col": 1})
        self.assertEqual(mapped["symbols"][0]["span"]["end"], {"line": 9, "col": 9})

    def test_a_fence_is_a_module_whose_edges_land_in_the_host(self):
        code = "from pkg.core import compute\n\n\ndef demo():\n    return compute(1)\n"
        text = "# Title\n\n```python\n" + code + "```\n"
        result = index(LIB, paths=[], regions=[host("docs/guide.md", text, code)])
        merged = fragment(result, "docs/guide.md")
        self.assertEqual([m["path"] for m in merged["modules"]], ["docs/.lighthouse-snippets/guide.md.1/snippet"])
        self.assertEqual(merged["symbols"][0]["file"], "docs/guide.md")
        self.assertEqual(merged["symbols"][0]["span"]["start"], {"line": 7, "col": 1})
        call = next(e for e in merged["edges"] if e["kind"] == "calls")
        self.assertEqual(call["to"], "pkg/core::compute#function")
        self.assertEqual(call["site"]["start"], {"line": 8, "col": 12})
        self.assertEqual(result["notices"], [])

    def test_a_fence_that_is_not_python_is_a_notice(self):
        code = ">>> compute(1)\n1\n"
        text = "```py\n" + code + "```\n"
        result = index(LIB, paths=[], regions=[host("README.md", text, code)])
        self.assertEqual(fragment(result, "README.md")["symbols"], [])
        self.assertTrue(result["notices"][0].startswith("README.md:2: the Python fence is not valid Python"))
        self.assertEqual(result["incomplete"], [])

    def test_names_the_prose_defines_degrade_to_syntactic_edges_with_a_notice(self):
        code = "def demo():\n    return compute(1)\n"
        text = "```python\n" + code + "```\n"
        result = index(LIB, paths=[], regions=[host("README.md", text, code)])
        found = edges(result, "README.md", "calls")
        self.assertEqual([e[2:] for e in found], [("pkg/core::compute#function", "syntactic")])
        self.assertTrue(any("matched by name" in n for n in result["notices"]))

    def test_a_fence_exports_only_what_its_own_all_lists(self):
        code = "__all__ = ['shown']\n\n\ndef shown():\n    pass\n\n\ndef hidden():\n    pass\n"
        text = "```python\n" + code + "```\n"
        merged = fragment(index(LIB, paths=[], regions=[host("README.md", text, code)]), "README.md")
        visibility = {s["name"]: s["visibility"] for s in merged["symbols"] if s["kind"] == "function"}
        self.assertEqual(visibility, {"shown": "public", "hidden": "private"})
        plain = "def demo():\n    pass\n"
        text = "```python\n" + plain + "```\n"
        merged = fragment(index(LIB, paths=[], regions=[host("README.md", text, plain)]), "README.md")
        self.assertEqual([s["visibility"] for s in merged["symbols"] if s["name"] == "demo"], ["private"])

    def test_comments_are_mapped_through_the_text_map(self):
        code = "# note\ndef f():\n    pass\n"
        quoted = "".join("> " + line + "\n" for line in code.splitlines())
        text = "```python\n" + quoted + "```\n"
        start = text.index("> ")
        segments = []
        text_at = 0
        host_at = start
        for line in code.splitlines(keepends=True):
            segments.append(
                {"text_range": [text_at, text_at + len(line)], "source_range": [host_at + 2, host_at + 2 + len(line)]}
            )
            text_at += len(line)
            host_at += 2 + len(line)
        region = host("README.md", text, code, start)
        region["sources"][0]["text_map"] = segments
        merged = fragment(index(LIB, paths=[], regions=[region]), "README.md")
        self.assertEqual(merged["comments"][0]["span"]["start"], {"line": 2, "col": 3})
        self.assertEqual(merged["comments"][0]["attached_to"], merged["symbols"][0]["id"])


if __name__ == "__main__":
    unittest.main()
