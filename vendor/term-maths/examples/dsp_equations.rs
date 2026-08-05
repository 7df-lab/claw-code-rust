use term_maths::render;

fn main() {
    let equations = [
        // DFT summation
        (
            r"X[k] = \sum_{n=0}^{N-1} x[n] \cdot e^{-j \frac{2\pi}{N} kn}",
            "DFT Summation",
        ),
        // Convolution integral
        (
            r"(f * g)(t) = \int_{-\infty}^{\infty} f(\tau) g(t - \tau) \, d\tau",
            "Convolution Integral",
        ),
        // Transfer function
        (
            r"H(z) = \frac{b_0 + b_1 z^{-1} + b_2 z^{-2}}{1 + a_1 z^{-1} + a_2 z^{-2}}",
            "Transfer Function",
        ),
        // Hann window
        (
            r"w(n) = 0.5 \left(1 - \cos\left(\frac{2\pi n}{N - 1}\right)\right)",
            "Hann Window",
        ),
    ];

    for (latex, label) in &equations {
        println!("=== {} ===", label);
        println!("LaTeX: {}", latex);
        println!();
        println!("{}", render(latex));
        println!();
    }

    // Also test individual components
    println!("=== Standalone tests ===\n");

    println!("--- Sum with limits ---");
    println!("{}\n", render(r"\sum_{n=0}^{N-1}"));

    println!("--- Integral with limits ---");
    println!("{}\n", render(r"\int_{0}^{1}"));

    println!("--- Product with limits ---");
    println!("{}\n", render(r"\prod_{i=1}^{n}"));

    println!("--- Delimited fraction ---");
    println!("{}\n", render(r"\left(\frac{a}{b}\right)"));

    println!("--- Overline ---");
    println!("{}\n", render(r"\overline{x + y}"));

    println!("--- Hat ---");
    println!("{}\n", render(r"\hat{x}"));

    println!("--- Sqrt of fraction ---");
    println!("{}\n", render(r"\sqrt{\frac{a}{b}}"));
}
