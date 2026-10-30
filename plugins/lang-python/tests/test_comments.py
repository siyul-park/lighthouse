import unittest

from lang_python.comments import Tokens
from lang_python.extent import Lines

SOURCE = "# first\n# second\ndef f():  # trailing\n    x = 1  # inner\n    return f'{x}' + 'a'\n\n# apart\n\ng = 2\n"


def tokens(text: str = SOURCE) -> Tokens:
    return Tokens(text, Lines(text))


class TokensTests(unittest.TestCase):
    def test_tokens_count(self):
        found = tokens()
        # `def f():` is five tokens; the comment after it is not one.
        self.assertEqual(found.count((3, 0), (4, 0)), 5)
        # A string with replacement fields is one token.
        self.assertEqual(found.count((5, 0), (6, 0)), 4)
        self.assertEqual(found.count((1, 0), (3, 0)), 0)

    def test_tokens_leading(self):
        found = tokens()
        self.assertEqual(found.leading(3).text, "# first")
        self.assertIsNone(found.leading(9))
        self.assertIsNone(found.leading(1))

    def test_tokens_trailing(self):
        found = tokens()
        self.assertEqual(found.trailing(3, 8).text, "# trailing")
        self.assertIsNone(found.trailing(3, 99))
        self.assertIsNone(found.trailing(1, 0))

    def test_tokens_grouped(self):
        grouped = tokens().grouped({3: "m::f#function", 9: "m::g#var"})
        self.assertEqual([c["text"] for c in grouped], ["# first\n# second", "# trailing", "# inner", "# apart"])
        self.assertEqual(grouped[0]["attached_to"], "m::f#function")
        self.assertNotIn("attached_to", grouped[1])
        self.assertNotIn("attached_to", grouped[3])
        self.assertEqual(grouped[0]["span"]["start"], {"line": 1, "col": 1})
        self.assertEqual(grouped[0]["span"]["end"], {"line": 2, "col": 9})

    def test_tokens_columns_are_utf8_bytes(self):
        found = tokens("é = 1  # é\n")
        self.assertEqual(found.comments[0].col, 8)


if __name__ == "__main__":
    unittest.main()
