"""The entry point: serves the plugin protocol on stdin and stdout."""

import sys

from lang_python.provider import Provider
from lang_python.sdk.server import serve

ID = "lang-python"
VERSION = "0.1.0"
MIN_VERSION = (3, 11)


def main() -> int:
    if sys.version_info < MIN_VERSION:
        found = sys.version_info[:2]
        sys.stderr.write("%s: Python %d.%d or newer is required, found %d.%d\n" % (ID, *MIN_VERSION, *found))
        return 1
    code = serve(Provider(ID, VERSION))
    if code != 0:
        sys.stderr.write("%s: input ended before shutdown and exit\n" % ID)
    return code
