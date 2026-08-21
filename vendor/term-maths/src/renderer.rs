//! Output backend trait and implementations.

use rust_latex_parser::EqNode;

use crate::rendered_block::RenderedBlock;

/// Trait for rendering an `EqNode` AST into a target output format.
pub trait MathRenderer {
    type Output;

    /// Render an equation AST node into the target output.
    fn render(&self, node: &EqNode) -> Self::Output;
}

/// Default renderer that produces a `RenderedBlock` (2D character grid).
/// Always available — no feature gates required.
pub struct TerminalRenderer;

impl MathRenderer for TerminalRenderer {
    type Output = RenderedBlock;

    fn render(&self, node: &EqNode) -> RenderedBlock {
        crate::layout::layout(node)
    }
}
