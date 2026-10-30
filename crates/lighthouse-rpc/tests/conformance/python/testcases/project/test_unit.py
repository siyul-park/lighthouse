import unittest

from calc import sub


class SubTests(unittest.TestCase):
    def setUp(self):
        self.value = 1

    def test_sub(self):
        self.assertEqual(sub(2, 1), self.value)

    def test_rows(self):
        for a, b in [(1, 1), (2, 2)]:
            with self.subTest(a=a):
                self.assertEqual(sub(a, b), 0)
