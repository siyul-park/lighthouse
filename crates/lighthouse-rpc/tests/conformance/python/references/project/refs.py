import functools


class Engine:
    def start(self):
        return 1

    def stop(self):
        return 2


class Boat:
    def start(self):
        return 3


def dynamic(thing, name):
    thing.stop()
    getattr(thing, "start")
    return thing.undefined(name)


def typed(engine: Engine):
    engine.start()
    engine.missing()


def inferred():
    engine = Engine()
    engine.stop()
    return engine


@functools.cache
def cached():
    return Engine().start()


def value():
    return Engine
