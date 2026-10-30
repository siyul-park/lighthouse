"""Models."""
import abc
import enum
import typing
from dataclasses import dataclass, field
from typing import NamedTuple, Optional, Protocol, TypedDict, overload


@dataclass
class Point:
    x: int
    y: int = 0
    tags: list = field(default_factory=list)
    label: Optional[str] = None


class Color(enum.Enum):
    RED = 1
    GREEN = 2
    _ignore_ = ["x"]

    def describe(self):
        return self.name


class Pair(NamedTuple):
    left: int
    right: int


class Row(TypedDict):
    key: str


class Shape(Protocol):
    def area(self) -> float: ...


class Base(abc.ABC):
    @abc.abstractmethod
    def run(self) -> None: ...

    def helper(self):
        return 1


class Square(Base):
    def run(self) -> None:
        return None

    def helper(self):
        return 2

    def area(self) -> float:
        return 4.0


class Circle(Shape):
    def area(self) -> float:
        return 3.0

    @typing.override
    def other(self):
        return 1


@overload
def parse(value: int) -> int: ...
@overload
def parse(value: str) -> str: ...
def parse(value):
    return value
