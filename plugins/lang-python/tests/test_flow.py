import ast
import unittest

from lang_python.flow import FlowWalker


def walk(source: str, recursive: str = "") -> FlowWalker:
    node = ast.parse(source).body[0]
    walker = FlowWalker(lambda call: isinstance(call.func, ast.Name) and call.func.id == recursive)
    walker.run(node.body)
    return walker


def kinds(walker: FlowWalker) -> list[tuple[str, int]]:
    return [(e["kind"], e["nesting"]) for e in walker.events]


class FlowWalkerTests(unittest.TestCase):
    def test_flow_walker_run(self):
        walker = walk(
            "def f(a, b):\n"
            "    if a:\n        for x in b:\n            pass\n"
            "    elif b:\n        pass\n"
            "    else:\n        while a:\n            pass\n"
        )
        self.assertEqual(
            kinds(walker),
            [("if", 0), ("loop", 1), ("else-if", 0), ("else", 0), ("loop", 1)],
        )
        self.assertEqual((walker.deepest, walker.count), (2, 6))

    def test_flow_walker_run_counts_a_def_without_its_docstring(self):
        walker = walk('def f():\n    """Docs."""\n    return 1\n')
        self.assertEqual((walker.count, walker.deepest, walker.events), (1, 0, []))

    def test_flow_walker_run_reads_an_expression_body(self):
        walker = FlowWalker(lambda call: False)
        walker.run(ast.parse("lambda: a if b else c", mode="eval").body.body)
        self.assertEqual((walker.count, kinds(walker)), (1, [("if", 0)]))

    def test_flow_walker_run_reads_logic_matches_exceptions_and_recursion(self):
        walker = walk(
            "def f(a, b):\n"
            "    x = a and b and (a or b)\n"
            "    try:\n        return f(a, b)\n    except E:\n        pass\n"
            "    match a:\n        case 1:\n            return 1\n        case _:\n            return 2\n",
            recursive="f",
        )
        found = kinds(walker)
        self.assertIn(("catch", 0), found)
        self.assertEqual([e for e in walker.events if e["kind"] == "logic"], [
            {"kind": "logic", "nesting": 0, "operators": 2},
            {"kind": "logic", "nesting": 0, "operators": 1},
        ])  # fmt: skip
        self.assertIn(("recursion", 0), found)
        switch = next(e for e in walker.events if e["kind"] == "switch")
        self.assertEqual((switch["arms"], switch.get("returning")), (1, True))

    def test_flow_walker_run_nests_inner_defs_and_classes(self):
        walker = walk(
            "def f():\n    def g():\n        if x:\n            pass\n"
            "    class C:\n        def h(self):\n            pass"
        )
        self.assertEqual(kinds(walker), [("if", 1)])
        self.assertEqual((walker.deepest, walker.count), (2, 6))


if __name__ == "__main__":
    unittest.main()
