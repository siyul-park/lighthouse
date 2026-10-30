import pytest

FIXTURE_ROWS = [1, 2]
DEFAULT = "x"


class Sample:
    value = 1


def assert_positive(n):
    assert n > 0


def make_sample():
    return Sample()


@pytest.fixture
def sample():
    return make_sample()


def test_it(sample):
    assert_positive(sample.value)
