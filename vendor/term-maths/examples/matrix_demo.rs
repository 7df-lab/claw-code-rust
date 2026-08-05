use term_maths::render;

fn main() {
    let examples = [
        (
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            "2x2 pmatrix",
        ),
        (
            r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}",
            "2x2 identity bmatrix",
        ),
        (
            r"\begin{vmatrix} a & b \\ c & d \end{vmatrix}",
            "2x2 determinant",
        ),
        (
            r"\begin{pmatrix} \frac{1}{2} & 0 \\ 0 & \frac{3}{4} \end{pmatrix}",
            "Matrix with fractions",
        ),
        (
            r"\begin{bmatrix} 1 & 2 & 3 \\ 4 & 5 & 6 \\ 7 & 8 & 9 \end{bmatrix}",
            "3x3 bmatrix",
        ),
        // Math font tests
        (r"\mathbb{R}", "Blackboard bold R"),
        (r"\mathbb{Z}", "Blackboard bold Z"),
        (r"\mathcal{L}", "Calligraphic L"),
        (r"\mathbf{x}", "Bold x"),
        (r"\mathfrak{g}", "Fraktur g"),
        (r"\mathbb{R}^n", "R^n"),
    ];

    for (latex, label) in &examples {
        println!("--- {} ---", label);
        println!("LaTeX: {}", latex);
        println!();
        println!("{}", render(latex));
        println!();
    }
}
