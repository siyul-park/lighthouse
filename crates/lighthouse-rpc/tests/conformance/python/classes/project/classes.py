"""Classes."""
import functools


def traced(fn):
    @functools.wraps(fn)
    def wrapper(*args):
        return fn(*args)

    return wrapper


class Outer:
    """Outer owns Inner."""

    class Inner:
        value = 1

        def read(self):
            return self.value

    scale = 2

    def __init__(self, name):
        self.name = name
        self._cache = {}

    @property
    def title(self):
        return self.name.title()

    @title.setter
    def title(self, value):
        self.name = value

    @staticmethod
    def make():
        return Outer("x")

    @classmethod
    def build(cls, name):
        return cls(name)

    @traced
    def show(self):
        inner = Outer.Inner()
        return inner.read() + self.scale

    def other(self, rival: "Outer"):
        return rival.show()


class Child(Outer):
    def show(self):
        return super().show() + self.scale
