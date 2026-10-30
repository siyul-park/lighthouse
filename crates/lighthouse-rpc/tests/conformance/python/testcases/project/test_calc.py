import pytest

from calc import Account, add, sub

ROWS = [(1, 2, 3), (2, 3, 5)]


def check_sum(a, b, expected):
    assert add(a, b) == expected


def build_rows():
    return ROWS


def test_add():
    assert add(1, 2) == 3


@pytest.mark.parametrize("a,b,expected", [(1, 2, 3), (2, 2, 4)])
def test_add_table(a, b, expected):
    check_sum(a, b, expected)


def test_loop():
    for a, b, expected in ROWS:
        assert add(a, b) == expected


def test_manual():
    if add(1, 1) != 2:
        raise AssertionError("bad")


class TestAccount:
    def test_deposit(self):
        assert Account().deposit(1) == 1

    def helper(self):
        return sub(1, 1)


class Holder:
    def test_ignored(self):
        return add(1, 1)


class TestNested:
    class TestInner:
        def test_deposit(self):
            assert Account().deposit(1) == 1
