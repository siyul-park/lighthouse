"""Extents."""
import functools

# leading comment
# second line
def lead():
    return 1  # trailing


# detached by a blank line

def detached():
    return 2


@functools.lru_cache
def decorated(x):
    """Doc of decorated.

    More text.
    """
    return x


LIMIT = 3
"""The limit."""


class Box:
    """A box."""

    # attribute comment
    size = 1  # trailing size

    def method(self):
        return self.size
