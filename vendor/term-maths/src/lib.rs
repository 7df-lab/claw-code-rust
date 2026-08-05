//! # term-maths
//!
//! Character-grid mathematical notation renderer for terminals.
//!
//! Accepts LaTeX math input and renders it as 2D Unicode character art suitable
//! for display in a terminal. Targets JuliaMono as the recommended font.
//!
//! ## Quick Start
//!
//! ```rust
//! let block = term_maths::render(r"\frac{a}{b}");
//! println!("{}", block);
//! //  a
//! // ───
//! //  b
//! ```
//!
//! ## Output Backends
//!
//! - **Plain text** — always available via [`render()`] and [`Display`](std::fmt::Display)
//! - **crossterm** — direct terminal output (feature `crossterm`)
//! - **ratatui** — TUI widget (feature `ratatui`)
//! - **LaTeX round-trip** — serialise back to LaTeX via [`to_latex()`]

pub mod latex_renderer;
pub mod layout;
pub mod mathfont;
pub mod rendered_block;
pub mod renderer;

#[cfg(feature = "crossterm")]
pub mod crossterm_renderer;

#[cfg(feature = "ratatui")]
pub mod ratatui_widget;

#[cfg(feature = "python")]
pub mod python;

pub use latex_renderer::LatexRenderer;
pub use rendered_block::RenderedBlock;
pub use renderer::{MathRenderer, TerminalRenderer};

#[cfg(feature = "crossterm")]
pub use crossterm_renderer::CrosstermRenderer;

#[cfg(feature = "ratatui")]
pub use ratatui_widget::MathWidget;

use rust_latex_parser::parse_equation;

/// Parse a LaTeX math string and render it as a 2D character grid.
///
/// This is the primary entry point for the library.
///
/// ```rust
/// let block = term_maths::render(r"x^2 + y^2 = z^2");
/// assert_eq!(format!("{}", block), "x² + y² = z²");
/// ```
pub fn render(latex: &str) -> RenderedBlock {
    let ast = parse_equation(latex);
    layout::layout(&ast)
}

/// Parse a LaTeX math string and serialise it back to LaTeX (round-trip).
///
/// Useful for normalising LaTeX input or for the LaTeX output backend.
pub fn to_latex(latex: &str) -> String {
    let ast = parse_equation(latex);
    let renderer = LatexRenderer;
    renderer.render(&ast)
}
