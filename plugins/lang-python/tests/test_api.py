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

    def test_comments_whitespace_and_docstrings_are_not_the_interface(self):
        def of(text):
            return api.digest(ast.parse(text), text)

        plain = "class C:\n    x = 1\n\ndef f(a):\n    return a\n"
        self.assertEqual(of(plain), of("# about\n" + plain.replace("x = 1", "x = 1  # one").replace("\n\n", "\n\n\n")))
        self.assertEqual(of(plain), of('"""The module."""\n' + plain.replace("x = 1", '"""The class."""\n    x = 1')))
        self.assertNotEqual(of(plain), of(plain.replace("x = 1", "x = 2")))
