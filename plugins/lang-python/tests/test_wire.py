import os
import subprocess
import sys
import unittest

from lang_python.project import declared_roots, decode

HERE = os.path.dirname(os.path.realpath(__file__))
SCRIPT = os.path.join(HERE, "..", "scripts", "wire.py")


class WireTests(unittest.TestCase):
    def test_the_generated_wire_types_are_fresh(self):
        done = subprocess.run([sys.executable, "-I", SCRIPT, "--check"], capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)


class ProjectTests(unittest.TestCase):
    def test_declared_package_dirs_of_setuptools_and_hatch(self):
        data = {
            "tool": {
                "setuptools": {"package-dir": {"": "lib"}},
                "hatch": {"build": {"targets": {"wheel": {"packages": ["python/pkg"]}}}},
            }
        }
        self.assertEqual(declared_roots(data), ["lib", "python"])

    def test_source_bytes_honour_coding_cookies_and_byte_order_marks(self):
        self.assertEqual(decode("﻿x = 1\r\n".encode("utf-8")), "x = 1\n")
        self.assertEqual(decode("# -*- coding: latin-1 -*-\nx = 'é'\n".encode("latin-1")).splitlines()[1], "x = 'é'")


if __name__ == "__main__":
    unittest.main()
