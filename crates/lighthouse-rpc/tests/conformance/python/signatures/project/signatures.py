from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from other import Thing


@dataclass
class Config:
    name: str
    parent: Optional[Config] = None
    retries: int = 3


class Service:
    def __init__(self, config: Config):
        self.config = config

    def fetch(self, key: str, *rest, limit: int = 3, **opts) -> Optional[Config]:
        return self.load(key, *rest, limit=limit, **opts)

    def load(self, key: str, *rest, limit: int = 3, **opts) -> Optional[Config]:
        return None

    @classmethod
    def create(cls, config: Config) -> Service:
        return cls(config)

    @staticmethod
    def pure(a, b) -> "Config":
        return Config(a)

    def wrapped(self, key):
        return self.load(key)

    def untyped(self, data):
        return data


def make(name: str) -> Config:
    return Config(name)


def forward(a, b):
    return make(a, b)


def passthrough(name: str):
    return make(name)


def thing(item: Thing, many: list[Config]) -> None:
    return None
