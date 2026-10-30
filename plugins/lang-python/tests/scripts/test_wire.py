import importlib.util
import io
import os
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.realpath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "scripts", "wire.py")


def load():
    """The generator script as a module; it is not a package member."""
    spec = importlib.util.spec_from_file_location("wire_script", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class WireTests(unittest.TestCase):
    def test_the_generated_wire_types_are_fresh(self):
        self.assertEqual(load().main(["--check"]), 0)

    def test_main_writes_the_module_and_checks_it_against_the_schema(self):
        wire = load()
        with tempfile.TemporaryDirectory() as directory:
            out = os.path.join(directory, "wire.py")
            with mock.patch.object(wire, "OUT", out):
                self.assertEqual(wire.main([]), 0)
                with open(out, encoding="utf-8") as handle:
                    written = handle.read()
                self.assertIn("IndexResult = TypedDict(", written)
                self.assertEqual(wire.main(["--check"]), 0)
                with open(out, "w", encoding="utf-8") as handle:
                    handle.write("# stale\n")
                with mock.patch.object(wire.sys, "stderr", io.StringIO()) as err:
                    self.assertEqual(wire.main(["--check"]), 1)
                self.assertIn("is stale", err.getvalue())

    def test_main(self):
        wire = load()
        self.assertEqual(wire.main(["--check"]), 0)


if __name__ == "__main__":
    unittest.main()
