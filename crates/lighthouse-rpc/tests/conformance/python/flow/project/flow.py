def classify(n, items):
    if n > 0 and n < 10 or n == 100:
        label = "small"
    elif n < 0:
        label = "neg"
    else:
        label = "other"
    for item in items:
        while item:
            item -= 1
            if item == 3:
                break
    else:
        label = "none"
    try:
        value = int(label)
    except ValueError:
        value = 0
    except (TypeError, KeyError):
        value = 1
    finally:
        label = ""
    return value if n else 0


def walk(node):
    if node is None:
        return 0
    return 1 + walk(node.left) + walk(node.right)


def dispatch(command):
    match command:
        case "a":
            return 1
        case "b":
            return 2
        case _:
            return 0


def mixed(command, other):
    match command:
        case "a":
            return other
        case _:
            print(other)


def nest():
    def inner():
        if True:
            return 1

    return inner
