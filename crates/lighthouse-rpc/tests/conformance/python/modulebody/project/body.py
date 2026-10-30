"""Runs things at load."""
import sys

VERSION = "1.0"


def setup():
    return 1


def main():
    return setup()


setup()

if __name__ == "__main__":
    sys.exit(main())
