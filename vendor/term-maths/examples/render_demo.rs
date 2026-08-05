use term_maths::render;

fn main() {
    let examples = [
        (r"\frac{a}{b}", "Simple fraction"),
        (r"\frac{1}{1+\frac{1}{x}}", "Nested fraction"),
        (r"x^2", "Superscript"),
        (r"a_n", "Subscript"),
        (r"x_i^2", "Super + subscript"),
        (r"a + b = c", "Sequence"),
        (r"e^{i\pi} + 1 = 0", "Euler's identity"),
        (r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}", "Quadratic formula"),
    ];

    for (latex, label) in &examples {
        println!("--- {} ---", label);
        println!("LaTeX: {}", latex);
        println!();
        println!("{}", render(latex));
        println!();
    }
}
