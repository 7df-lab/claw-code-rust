//! Generates Python type stub files (.pyi) for the term_maths extension module.
//!
//! Run with:
//!
//! ```sh
//! cargo run --features python --bin stub_gen
//! ```
//!
//! The stubs are written to `python/term_maths/_term_maths.pyi` (relative to
//! the workspace root).  The output path is determined automatically by
//! pyo3-stub-gen by scanning upward for `pyproject.toml`.

fn main() {
    let stub = term_maths::python::stub_info_gatherer().expect("Failed to collect stub info");
    stub.generate().expect("Failed to generate Python stubs");
}
