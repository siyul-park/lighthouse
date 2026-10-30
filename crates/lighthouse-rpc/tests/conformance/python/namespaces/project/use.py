import ns.a.mod
from ns.b import other


def use():
    return ns.a.mod.alpha() + other.beta()
