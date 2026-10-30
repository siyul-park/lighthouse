def exposed():
    return 1


def _lifted():
    return 2


def _hidden():
    return 3


class _Quiet:
    def visible(self):
        return 1

    def _tucked(self):
        return 2

    def __repr__(self):
        return "q"
