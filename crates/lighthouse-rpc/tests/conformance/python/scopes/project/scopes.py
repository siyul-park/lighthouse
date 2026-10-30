"""Nested scopes, closures and nonlocal."""

counter = 0


def helper() -> int:
    return 1


def outer(values):
    total = helper()

    def inner(step):
        nonlocal total
        total += step
        return helper()

    def shadow():
        helper = 3
        return helper

    squares = [helper() + v for v in values if v]
    pick = lambda item: helper() + item  # noqa: E731
    inner(2)
    return total, squares, pick, shadow


def bump():
    global counter
    counter += helper()
    return counter


class Holder:
    size = helper()

    def method(self):
        return [helper() for _ in range(self.size)]
