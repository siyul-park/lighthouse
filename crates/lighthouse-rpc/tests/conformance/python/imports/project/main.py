import os
import numpy
import app.sub.leaf as leaf
from app import util
from app.sub import Leaf
from app.util import double as twice
from .missing import ghost


def main():
    os.getcwd()
    numpy.zeros(1)
    leaf.Leaf().go()
    util.triple(2)
    Leaf().go()
    return twice(3), ghost
