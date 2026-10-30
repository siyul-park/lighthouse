class Node:
    next: "Node | None" = None

    def link(self, other: "Node") -> "Node":
        return other


def build(first: "Node", second: Node) -> "list[Node]":
    return [first, second]
