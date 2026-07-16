"""Milestone 0 exit criteria: the package installs, imports, and its two
halves (Python surface, Rust core) agree — on GIL and free-threaded builds."""

import sys
import sysconfig

import rayo
from rayo._core import core_version


def test_app_class_is_importable() -> None:
    application = rayo.Rayo(title="smoke test")
    assert application.title == "smoke test"


def test_version_comes_from_the_rust_core() -> None:
    assert rayo.__version__ == core_version()
    assert rayo.__version__.count(".") == 2


def test_free_threaded_build_does_not_reenable_the_gil() -> None:
    """On 3.13t+ builds, importing rayo must not silently re-enable the GIL —
    that would mean the extension was not declared `gil_used = false`."""
    if not sysconfig.get_config_var("Py_GIL_DISABLED"):
        return  # GIL build: nothing to check
    assert not sys._is_gil_enabled()
