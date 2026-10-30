import unittest
from typing import Any

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


class RegionTests(unittest.TestCase):
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
