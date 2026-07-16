//! `rayo-core` — the PyO3 extension module that binds Rayo's Rust crates to
//! Python as `rayo._core`.
//!
//! This crate is the only place Python and Rust meet. The boundary rules are
//! hard invariants (ARCHITECTURE.md §6): one `Python::attach` per request,
//! `detach` around Rust work longer than 1 ms, and zero Python object
//! creation on the response serialization path.

use pyo3::prelude::*;

/// The version compiled into the Rust core. The Python layer checks this
/// against the installed package version at import time and fails loudly on
/// mismatch (invariant 5: nothing defers a broken install to request time).
#[pyfunction]
fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `rayo._core` — declared `gil_used = false`: nothing in this module may
/// rely on the GIL for correctness (ADR-0002, invariant 4).
#[pymodule(gil_used = false)]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(core_version, module)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_matches_cargo_manifest() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }
}
