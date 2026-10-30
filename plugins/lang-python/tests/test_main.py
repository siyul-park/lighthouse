import io
import unittest
from collections import namedtuple
from unittest import mock

from lang_python import main as entry

OLD = (3, 10, 0, "final", 0)


class MainTests(unittest.TestCase):
    def test_main(self):
        with (
            mock.patch.object(entry, "serve", return_value=0) as serve,
            mock.patch.object(entry.sys, "stderr", io.StringIO()) as err,
        ):
            self.assertEqual(entry.main(), 0)
        self.assertEqual(serve.call_args.args[0].name, "lang-python")
        self.assertEqual(err.getvalue(), "")

    def test_main_reports_input_that_ends_before_shutdown(self):
        with (
            mock.patch.object(entry, "serve", return_value=1),
            mock.patch.object(entry.sys, "stderr", io.StringIO()) as err,
        ):
            self.assertEqual(entry.main(), 1)
        self.assertIn("input ended before shutdown and exit", err.getvalue())

    def test_main_refuses_an_interpreter_that_is_too_old(self):
        info = namedtuple("info", "major minor micro releaselevel serial")(*OLD)
        with mock.patch.object(entry.sys, "version_info", info), mock.patch.object(entry, "serve") as serve:
            with mock.patch.object(entry.sys, "stderr", io.StringIO()) as err:
                self.assertEqual(entry.main(), 1)
        serve.assert_not_called()
        self.assertIn("Python 3.11 or newer is required, found 3.10", err.getvalue())


if __name__ == "__main__":
    unittest.main()
