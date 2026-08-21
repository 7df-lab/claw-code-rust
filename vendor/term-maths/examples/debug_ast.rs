use rust_latex_parser::parse_equation;

fn main() {
    for expr in std::env::args().skip(1) {
        println!("=== {} ===", expr);
        println!("{:#?}", parse_equation(&expr));
        println!();
    }
    if std::env::args().len() <= 1 {
        // Defaults
        for expr in [
            r"a + b = c",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        ] {
            println!("=== {} ===", expr);
            println!("{:#?}", parse_equation(expr));
            println!();
        }
    }
}
