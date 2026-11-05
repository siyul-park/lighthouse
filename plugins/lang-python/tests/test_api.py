import ast
import unittest

from lang_python import api


class ApiTests(unittest.TestCase):
    def test_digest(self):
        def of(text):
            return api.digest(ast.parse(text), text)

        self.assertEqual(of("def f(x):\n    return x\n"), of("def f(x):\n    return x + 1\n"))
        self.assertNotEqual(of("def f(x):\n    pass\n"), of("def f(x, y):\n    pass\n"))
        self.assertNotEqual(
            of("class C:\n    def m(self):\n        self.a = 1\n"),
            of("class C:\n    def m(self):\n        self.b = 1\n"),
        )
        self.assertEqual(
            of("class C:\n    def m(self):\n        return 1\n"),
            of("class C:\n    def m(self):\n        return 2\n"),
        )
        self.assertEqual(
            of("class C:\n    def m(self):\n        self.a = 1\n        print(1)\n"),
            of("class C:\n    def m(self):\n        self.a = 1\n        print(2)\n"),
        )
        self.assertNotEqual(
            of("class C:\n    def m(self):\n        self.a = A()\n"),
            of("class C:\n    def m(self):\n        self.a = B()\n"),
        )
        self.assertNotEqual(
            of("class C:\n    def m(self, x):\n        return x\n"),
            of("class C:\n    def m(self):\n        return 1\n"),
        )
        self.assertNotEqual(api.digest(None, "def f(:\n"), api.digest(None, "def g(:\n"))
