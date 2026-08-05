//! Demonstrates the LaTeX renderer (round-trip serialisation).
//!
//! Run with: cargo run --example latex_roundtrip

use term_maths::{render, to_latex};

fn main() {
    let examples = [
        r"\frac{a}{b}",
        r"x^2 + y^2 = z^2",
        r"\sum_{i=1}^{n} x_i",
        r"\sqrt{b^2 - 4ac}",
        r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        r"\mathbb{R}^n",
    ];

    for latex in &examples {
        println!("Original:    {}", latex);
        let roundtrip = to_latex(latex);
        println!("Round-trip:  {}", roundtrip.trim());
        println!("Rendered:");
        println!("{}", render(latex));
        println!();
    }
}
