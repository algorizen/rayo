"""Rayo — the web framework for free-threaded Python. Rust core, Python surface."""

from importlib import metadata

from rayo._core import core_version

_compiled_core_version = core_version()

try:
    _installed_package_version = metadata.version("rayo")
except metadata.PackageNotFoundError:
    # Editable/in-tree use without installed metadata: the core is the only truth.
    _installed_package_version = _compiled_core_version

if _installed_package_version != _compiled_core_version:
    raise ImportError(
        f"This rayo installation is broken: the Python package is version "
        f"{_installed_package_version!r} but its compiled Rust core reports "
        f"{_compiled_core_version!r}. This usually means a partial upgrade. "
        f"Fix: reinstall with `pip install --force-reinstall rayo` "
        f"(or `uv pip install --force-reinstall rayo`)."
    )

__version__ = _compiled_core_version

from rayo._app import Rayo  # noqa: E402 — the version consistency check must run first

__all__ = ["Rayo", "__version__"]
