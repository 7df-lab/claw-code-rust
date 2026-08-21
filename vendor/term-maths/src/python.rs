//! Python bindings for term-maths (enabled with the `python` feature).
//!
//! This module exposes the core API as a Python extension module named `_term_maths`.
//! It is intended to be imported through the `term_maths` Python package, which
//! re-exports everything from this compiled extension.
//!
//! ## Python usage
//!
//! ```python
//! import term_maths
//!
//! block = term_maths.render(r"\frac{a}{b}")
//! print(block)          # multi-line Unicode art
//! print(block.width)    # int
//! print(block.height)   # int
//! print(block.baseline) # int
//! print(block.cells())  # list[list[str]]
//! ```

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_stub_gen::define_stub_info_gatherer;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};

use crate::RenderedBlock;

// ---------------------------------------------------------------------------
// RenderedBlock Python wrapper
// ---------------------------------------------------------------------------

/// A rectangular character grid produced by rendering a LaTeX math expression.
///
/// Each cell contains one terminal column's worth of text. The :attr:`baseline`
/// marks the row used for horizontal alignment when composing blocks side-by-side.
///
/// Construct via the module-level :func:`render` function or the static
/// constructors (:meth:`from_char`, :meth:`from_text`, :meth:`empty`,
/// :meth:`hline`).
#[gen_stub_pyclass]
#[pyclass(name = "RenderedBlock", module = "term_maths")]
pub struct PyRenderedBlock(pub RenderedBlock);

#[gen_stub_pymethods]
#[pymethods]
impl PyRenderedBlock {
    // ------------------------------------------------------------------
    // Static constructors
    // ------------------------------------------------------------------

    /// Create a block containing a single character.
    ///
    /// :param ch: A single Unicode character.
    /// :type ch: str
    /// :raises ValueError: If ``ch`` is not exactly one character.
    #[staticmethod]
    fn from_char(ch: &str) -> PyResult<PyRenderedBlock> {
        let c = ch.chars().next().ok_or_else(|| {
            PyValueError::new_err("from_char expects a single character, got an empty string")
        })?;
        Ok(PyRenderedBlock(RenderedBlock::from_char(c)))
    }

    /// Create a single-row block from a text string.
    ///
    /// :param text: The text to render.
    /// :type text: str
    #[staticmethod]
    fn from_text(text: &str) -> PyRenderedBlock {
        PyRenderedBlock(RenderedBlock::from_text(text))
    }

    /// Create an empty block with zero dimensions.
    #[staticmethod]
    fn empty() -> PyRenderedBlock {
        PyRenderedBlock(RenderedBlock::empty())
    }

    /// Create a horizontal line of a given character repeated *width* times.
    ///
    /// :param ch: The character to repeat (e.g. ``'─'``).
    /// :type ch: str
    /// :param width: Number of columns.
    /// :type width: int
    /// :raises ValueError: If ``ch`` is not exactly one character.
    #[staticmethod]
    fn hline(ch: &str, width: usize) -> PyResult<PyRenderedBlock> {
        let c = ch.chars().next().ok_or_else(|| {
            PyValueError::new_err("hline expects a single character, got an empty string")
        })?;
        Ok(PyRenderedBlock(RenderedBlock::hline(c, width)))
    }

    // ------------------------------------------------------------------
    // Properties
    // ------------------------------------------------------------------

    /// Width of the block in terminal columns.
    #[getter]
    fn width(&self) -> usize {
        self.0.width()
    }

    /// Height of the block in rows.
    #[getter]
    fn height(&self) -> usize {
        self.0.height()
    }

    /// Row index (0-indexed from top) used as the alignment baseline.
    #[getter]
    fn baseline(&self) -> usize {
        self.0.baseline()
    }

    // ------------------------------------------------------------------
    // Methods
    // ------------------------------------------------------------------

    /// Return the cell grid as a list of rows, where each row is a list of
    /// single-column strings.
    ///
    /// :rtype: list[list[str]]
    fn cells(&self) -> Vec<Vec<String>> {
        self.0.cells().to_vec()
    }

    /// Return ``True`` if the block has zero width or height.
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Place *other* immediately to the right of *self*, aligning on baselines.
    ///
    /// Shorter blocks are padded with empty rows above or below as needed.
    ///
    /// :param other: The block to append on the right.
    /// :type other: RenderedBlock
    /// :rtype: RenderedBlock
    fn beside(&self, other: &PyRenderedBlock) -> PyRenderedBlock {
        PyRenderedBlock(self.0.beside(&other.0))
    }

    /// Stack *top* above *bottom* and set the baseline to *baseline_row*.
    ///
    /// :param top: Upper block.
    /// :type top: RenderedBlock
    /// :param bottom: Lower block.
    /// :type bottom: RenderedBlock
    /// :param baseline_row: Row index (in the combined block) for the baseline.
    /// :type baseline_row: int
    /// :rtype: RenderedBlock
    #[staticmethod]
    fn above(
        top: &PyRenderedBlock,
        bottom: &PyRenderedBlock,
        baseline_row: usize,
    ) -> PyRenderedBlock {
        PyRenderedBlock(RenderedBlock::above(&top.0, &bottom.0, baseline_row))
    }

    /// Add empty space around the block.
    ///
    /// :param left: Columns to add on the left.
    /// :param right: Columns to add on the right.
    /// :param top: Rows to add on top.
    /// :param bottom: Rows to add on the bottom.
    /// :rtype: RenderedBlock
    fn pad(&self, left: usize, right: usize, top: usize, bottom: usize) -> PyRenderedBlock {
        PyRenderedBlock(self.0.pad(left, right, top, bottom))
    }

    /// Horizontally centre the block within a target width.
    ///
    /// If *target_width* is not larger than the current width, returns a clone.
    ///
    /// :param target_width: Desired total width in columns.
    /// :type target_width: int
    /// :rtype: RenderedBlock
    fn center_in(&self, target_width: usize) -> PyRenderedBlock {
        PyRenderedBlock(self.0.center_in(target_width))
    }

    // ------------------------------------------------------------------
    // Dunder methods
    // ------------------------------------------------------------------

    fn __str__(&self) -> String {
        format!("{}", self.0)
    }

    fn __repr__(&self) -> String {
        format!(
            "RenderedBlock(width={}, height={}, baseline={})",
            self.0.width(),
            self.0.height(),
            self.0.baseline(),
        )
    }
}

// ---------------------------------------------------------------------------
// Helper: parse font kind from string
// ---------------------------------------------------------------------------

fn parse_font_kind(font: &str) -> PyResult<rust_latex_parser::MathFontKind> {
    use rust_latex_parser::MathFontKind;
    match font {
        "bold" => Ok(MathFontKind::Bold),
        "blackboard" => Ok(MathFontKind::Blackboard),
        "calligraphic" => Ok(MathFontKind::Calligraphic),
        "fraktur" => Ok(MathFontKind::Fraktur),
        "roman" => Ok(MathFontKind::Roman),
        "sans_serif" => Ok(MathFontKind::SansSerif),
        "monospace" => Ok(MathFontKind::Monospace),
        other => Err(PyValueError::new_err(format!(
            "Unknown font kind {other:?}. \
             Valid options: bold, blackboard, calligraphic, fraktur, roman, sans_serif, monospace"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Python module definition (inline style — required for experimental-inspect)
// ---------------------------------------------------------------------------

/// Python extension module ``_term_maths``.
///
/// Import via the ``term_maths`` package rather than directly:
///
/// .. code-block:: python
///
///    import term_maths
///    block = term_maths.render(r"\frac{a}{b}")
#[pymodule]
pub mod _term_maths {
    use super::*;

    // Re-export the class so pyclass metadata is visible to the module.
    #[pymodule_export]
    use super::PyRenderedBlock;

    /// Parse a LaTeX math string and render it as a 2D character grid.
    ///
    /// This is the primary entry point of the library.
    ///
    /// :param latex: A LaTeX math expression (without surrounding ``$`` delimiters).
    /// :type latex: str
    /// :returns: The rendered block.
    /// :rtype: RenderedBlock
    ///
    /// Example:
    ///
    /// ```python
    /// >>> import term_maths
    /// >>> print(term_maths.render(r"\frac{a}{b}"))
    ///  a
    /// ───
    ///  b
    /// ```
    #[pyfunction]
    pub fn render(latex: &str) -> PyRenderedBlock {
        PyRenderedBlock(crate::render(latex))
    }

    /// Parse a LaTeX math string and serialise it back to normalised LaTeX.
    ///
    /// Useful for round-tripping or canonicalising LaTeX input.
    ///
    /// :param latex: A LaTeX math expression.
    /// :type latex: str
    /// :rtype: str
    #[pyfunction]
    pub fn to_latex(latex: &str) -> String {
        crate::to_latex(latex)
    }

    /// Map a single character to its Unicode mathematical font variant.
    ///
    /// Returns the original character unchanged if no mapping exists for
    /// the given font kind.
    ///
    /// :param font: One of ``"bold"``, ``"blackboard"``, ``"calligraphic"``,
    ///     ``"fraktur"``, ``"roman"``, ``"sans_serif"``, ``"monospace"``.
    /// :type font: str
    /// :param ch: A single Unicode character.
    /// :type ch: str
    /// :rtype: str
    /// :raises ValueError: If *font* is not a recognised font kind, or *ch* is empty.
    #[pyfunction]
    pub fn map_char(font: &str, ch: &str) -> PyResult<String> {
        let kind = parse_font_kind(font)?;
        let c = ch.chars().next().ok_or_else(|| {
            PyValueError::new_err("map_char expects a single character, got an empty string")
        })?;
        Ok(crate::mathfont::map_char(&kind, c).to_string())
    }

    /// Map every character in a string to its Unicode mathematical font variant.
    ///
    /// Characters without a mapping are passed through unchanged.
    ///
    /// :param font: One of ``"bold"``, ``"blackboard"``, ``"calligraphic"``,
    ///     ``"fraktur"``, ``"roman"``, ``"sans_serif"``, ``"monospace"``.
    /// :type font: str
    /// :param s: The string to transform.
    /// :type s: str
    /// :rtype: str
    /// :raises ValueError: If *font* is not a recognised font kind.
    #[pyfunction]
    pub fn map_str(font: &str, s: &str) -> PyResult<String> {
        let kind = parse_font_kind(font)?;
        Ok(crate::mathfont::map_str(&kind, s))
    }
}

// ---------------------------------------------------------------------------
// Stub generation entry point (used by src/bin/stub_gen.rs)
// ---------------------------------------------------------------------------

define_stub_info_gatherer!(stub_info_gatherer);
