use term_maths::render;

/// Helper: render and collect output lines, trimming trailing whitespace per line.
fn render_lines(latex: &str) -> Vec<String> {
    let block = render(latex);
    let output = format!("{}", block);
    output.lines().map(|l| l.trim_end().to_string()).collect()
}

#[test]
fn test_simple_fraction() {
    let lines = render_lines(r"\frac{a}{b}");
    assert_eq!(lines, vec![" a", "───", " b"]);
}

#[test]
fn test_nested_fraction() {
    let lines = render_lines(r"\frac{1}{1+\frac{1}{x}}");
    // Numerator "1" centered over denominator "1 + 1/x"
    assert_eq!(lines.len(), 5);
    // Top line: centered "1"
    assert!(lines[0].contains('1'));
    // Bar line
    assert!(lines[1].chars().all(|c| c == '─'));
    // Denominator contains nested fraction
    assert!(lines[3].contains('+'));
}

#[test]
fn test_superscript() {
    // Simple digits use inline Unicode superscript
    let lines = render_lines(r"x^2");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "x²");
}

#[test]
fn test_subscript() {
    // Simple chars use inline Unicode subscript
    let lines = render_lines(r"a_n");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "aₙ");
}

#[test]
fn test_supsub() {
    // Both scripts inline when possible
    let lines = render_lines(r"x_i^2");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "x²ᵢ");
}

#[test]
fn test_superscript_fallback() {
    // Complex superscripts fall back to multi-row
    let lines = render_lines(r"e^{i\pi}");
    assert!(lines.len() >= 2);
    let joined = lines.join("\n");
    assert!(joined.contains('π'));
    assert!(joined.contains('e'));
}

#[test]
fn test_horizontal_sequence() {
    let block = render(r"a + b");
    // Should be a single row with spaces around operators
    assert_eq!(block.height(), 1);
    let output = format!("{}", block);
    assert!(output.contains("a"));
    assert!(output.contains("+"));
    assert!(output.contains("b"));
}

#[test]
fn test_sqrt() {
    let lines = render_lines(r"\sqrt{x}");
    // Should have overline and radical
    assert!(lines[0].contains('─'));
    assert!(lines.iter().any(|l| l.contains('√')));
    assert!(lines.iter().any(|l| l.contains('x')));
}

#[test]
fn test_euler_identity() {
    let lines = render_lines(r"e^{i\pi} + 1 = 0");
    // Should render as 2 rows (e with superscript iπ, then + 1 = 0)
    assert!(lines.len() >= 2);
    // Top row should have iπ
    let joined = lines.join("\n");
    assert!(joined.contains('π'));
    assert!(joined.contains('0'));
}

#[test]
fn test_quadratic_formula() {
    let lines = render_lines(r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}");
    // Should be a multi-line fraction with sqrt in numerator
    assert!(lines.len() >= 3); // at least num + bar + den
    // Contains the fraction bar
    assert!(lines.iter().any(|l| l.contains('─') && !l.contains('√')));
    // Contains sqrt
    let joined = lines.join("\n");
    assert!(joined.contains('√'));
    assert!(joined.contains("2a"));
}

#[test]
fn test_empty_input() {
    let block = render("");
    assert!(block.is_empty() || block.height() <= 1);
}

#[test]
fn test_single_symbol() {
    let block = render(r"\alpha");
    assert_eq!(block.height(), 1);
    let output = format!("{}", block);
    assert!(output.contains('α'));
}

#[test]
fn test_fraction_baseline_alignment() {
    // When a fraction appears beside other content, baselines should align
    let lines = render_lines(r"x + \frac{a}{b}");
    // x and + should be on the fraction bar row
    let bar_row = lines.iter().position(|l| l.contains('─')).unwrap();
    assert!(lines[bar_row].contains('x') || lines[bar_row].contains('+'));
}

// ── Sprint 2: DSP Reference Equations ──────────────────────────────────

#[test]
fn test_dft_summation() {
    // X[k] = Σ_{n=0}^{N-1} x[n] · e^{-j 2π/N kn}
    let lines = render_lines(r"X[k] = \sum_{n=0}^{N-1} x[n] \cdot e^{-j \frac{2\pi}{N} kn}");
    let joined = lines.join("\n");

    // Must contain the summation symbol
    assert!(joined.contains('∑'), "missing Σ");
    // Must contain upper limit N-1 and lower limit n=0
    assert!(
        joined.contains("N - 1") || joined.contains("N-1"),
        "missing upper limit"
    );
    assert!(
        joined.contains("n = 0") || joined.contains("n=0"),
        "missing lower limit"
    );
    // Must contain the exponent's fraction 2π/N
    assert!(joined.contains('π'), "missing π in exponent");
    // Multi-line output
    assert!(lines.len() >= 3, "DFT should be at least 3 lines tall");
}

#[test]
fn test_convolution_integral() {
    // (f * g)(t) = ∫_{-∞}^{∞} f(τ) g(t - τ) dτ
    let lines = render_lines(r"(f * g)(t) = \int_{-\infty}^{\infty} f(\tau) g(t - \tau) \, d\tau");
    let joined = lines.join("\n");

    // Must contain integral pieces
    assert!(
        joined.contains('⌠') || joined.contains('∫'),
        "missing integral symbol"
    );
    // Must contain limits
    assert!(joined.contains('∞'), "missing infinity");
    assert!(joined.contains("-∞"), "missing negative infinity");
    // Must contain tau
    assert!(joined.contains('τ'), "missing tau");
    // Multi-line output (integral is 3+ rows)
    assert!(
        lines.len() >= 3,
        "convolution integral should be at least 3 lines"
    );
}

#[test]
fn test_transfer_function() {
    // H(z) = (b₀ + b₁z⁻¹ + b₂z⁻²) / (1 + a₁z⁻¹ + a₂z⁻²)
    let lines =
        render_lines(r"H(z) = \frac{b_0 + b_1 z^{-1} + b_2 z^{-2}}{1 + a_1 z^{-1} + a_2 z^{-2}}");
    let joined = lines.join("\n");

    // Must contain fraction bar
    assert!(
        lines.iter().any(|l| l.contains('─')),
        "missing fraction bar"
    );
    // Must contain subscripted coefficients
    assert!(joined.contains('₀') || joined.contains("b_0"), "missing b₀");
    assert!(joined.contains('₁') || joined.contains("b_1"), "missing b₁");
    // Must contain z⁻¹ (inline superscript)
    assert!(joined.contains("z⁻¹"), "missing z⁻¹");
    // Three lines minimum (num + bar + den)
    assert!(
        lines.len() >= 3,
        "transfer function should be at least 3 lines"
    );
}

#[test]
fn test_hann_window() {
    // w(n) = 0.5(1 - cos(2πn / (N-1)))
    let lines = render_lines(r"w(n) = 0.5 \left(1 - \cos\left(\frac{2\pi n}{N - 1}\right)\right)");
    let joined = lines.join("\n");

    // Must contain cos
    assert!(joined.contains("cos"), "missing cos");
    // Must contain π
    assert!(joined.contains('π'), "missing π");
    // Must contain scaled delimiters
    assert!(
        joined.contains('⎛') || joined.contains('('),
        "missing delimiter"
    );
    // Must contain N - 1 in denominator
    assert!(joined.contains("N - 1"), "missing N - 1 denominator");
    // Multi-line (fraction inside delimiters)
    assert!(lines.len() >= 2, "Hann window should be at least 2 lines");
}

// ── Sprint 2: Component Tests ──────────────────────────────────────────

#[test]
fn test_integral_multirow() {
    let lines = render_lines(r"\int_{0}^{1}");
    let joined = lines.join("\n");
    // Should use multi-row integral characters
    assert!(joined.contains('⌠'), "missing ⌠ top piece");
    assert!(joined.contains('⌡'), "missing ⌡ bottom piece");
}

#[test]
fn test_sum_with_limits() {
    let lines = render_lines(r"\sum_{i=1}^{n}");
    let joined = lines.join("\n");
    assert!(joined.contains('∑'), "missing Σ");
    // Upper limit above, lower limit below
    assert!(
        lines.len() >= 3,
        "sum with limits should be at least 3 lines"
    );
}

#[test]
fn test_scaled_delimiters() {
    let lines = render_lines(r"\left(\frac{a}{b}\right)");
    let joined = lines.join("\n");
    // Should use bracket piece characters for 3-row fraction
    assert!(
        joined.contains('⎛') && joined.contains('⎝'),
        "missing scaled parentheses"
    );
    assert!(joined.contains('─'), "missing fraction bar");
}

#[test]
fn test_overline() {
    let lines = render_lines(r"\overline{abc}");
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains('‾'), "missing overline character");
    assert!(lines[1].contains("abc"), "missing body");
}

#[test]
fn test_accent_hat() {
    let lines = render_lines(r"\hat{x}");
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains('^'), "missing hat");
    assert!(lines[1].contains('x'), "missing body");
}

#[test]
fn test_accent_vec() {
    let lines = render_lines(r"\vec{v}");
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains('→'), "missing arrow");
    assert!(lines[1].contains('v'), "missing body");
}

#[test]
fn test_sqrt_of_fraction() {
    let lines = render_lines(r"\sqrt{\frac{a}{b}}");
    let joined = lines.join("\n");
    assert!(joined.contains('√'), "missing radical");
    assert!(joined.contains('─'), "missing overline or fraction bar");
    assert!(lines.len() >= 3, "sqrt of fraction should be multi-line");
}

// ── Sprint 3: Matrix and Symbol Coverage ───────────────────────────────

#[test]
fn test_pmatrix_2x2() {
    let lines = render_lines(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}");
    let joined = lines.join("\n");
    // Parenthesis delimiters
    assert!(
        joined.contains('⎛') || joined.contains('('),
        "missing left paren"
    );
    assert!(
        joined.contains('⎞') || joined.contains(')'),
        "missing right paren"
    );
    // All entries present
    for ch in ['a', 'b', 'c', 'd'] {
        assert!(joined.contains(ch), "missing entry {}", ch);
    }
}

#[test]
fn test_bmatrix_identity() {
    let lines = render_lines(r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}");
    let joined = lines.join("\n");
    // Bracket delimiters
    assert!(
        joined.contains('⎡') || joined.contains('['),
        "missing left bracket"
    );
    assert!(
        joined.contains('⎤') || joined.contains(']'),
        "missing right bracket"
    );
}

#[test]
fn test_vmatrix_determinant() {
    let lines = render_lines(r"\begin{vmatrix} a & b \\ c & d \end{vmatrix}");
    let joined = lines.join("\n");
    assert!(joined.contains('│'), "missing vertical bar delimiter");
}

#[test]
fn test_matrix_with_fractions() {
    let lines = render_lines(r"\begin{pmatrix} \frac{1}{2} & 0 \\ 0 & \frac{3}{4} \end{pmatrix}");
    let joined = lines.join("\n");
    // Fraction bars inside cells
    assert!(joined.contains('─'), "missing fraction bar");
    // Both fractions present
    assert!(joined.contains('1') && joined.contains('2'), "missing 1/2");
    assert!(joined.contains('3') && joined.contains('4'), "missing 3/4");
    // Multi-line (fractions make rows taller)
    assert!(
        lines.len() >= 4,
        "matrix with fractions should be at least 4 lines"
    );
}

#[test]
fn test_3x3_matrix() {
    let lines = render_lines(r"\begin{bmatrix} 1 & 2 & 3 \\ 4 & 5 & 6 \\ 7 & 8 & 9 \end{bmatrix}");
    let joined = lines.join("\n");
    // All digits present
    for d in '1'..='9' {
        assert!(joined.contains(d), "missing digit {}", d);
    }
    assert!(lines.len() >= 3, "3x3 matrix should be at least 3 lines");
}

// ── Math Font Tests ────────────────────────────────────────────────────

#[test]
fn test_mathbb_double_struck() {
    let block = render(r"\mathbb{R}");
    let output = format!("{}", block);
    assert_eq!(output.trim(), "ℝ");
}

#[test]
fn test_mathbb_integers() {
    let block = render(r"\mathbb{Z}");
    let output = format!("{}", block);
    assert_eq!(output.trim(), "ℤ");
}

#[test]
fn test_mathcal_script() {
    let block = render(r"\mathcal{L}");
    let output = format!("{}", block);
    assert_eq!(output.trim(), "ℒ");
}

#[test]
fn test_mathbf_bold() {
    let block = render(r"\mathbf{x}");
    let output = format!("{}", block);
    assert_eq!(output.trim(), "𝐱");
}

#[test]
fn test_mathfrak_fraktur() {
    let block = render(r"\mathfrak{g}");
    let output = format!("{}", block);
    assert_eq!(output.trim(), "𝔤");
}

#[test]
fn test_mathbb_with_superscript() {
    let lines = render_lines(r"\mathbb{R}^n");
    assert_eq!(lines.len(), 1);
    let output = &lines[0];
    assert!(output.contains('ℝ'), "missing double-struck R");
    assert!(output.contains('ⁿ'), "missing superscript n");
}

#[test]
fn test_mathsf_sans_serif() {
    let block = render(r"\mathsf{ABC}");
    let output = format!("{}", block);
    assert!(output.contains('𝖠'), "missing sans-serif A");
    assert!(output.contains('𝖡'), "missing sans-serif B");
    assert!(output.contains('𝖢'), "missing sans-serif C");
}

#[test]
fn test_mathtt_monospace() {
    let block = render(r"\mathtt{code}");
    let output = format!("{}", block);
    assert!(output.contains('𝚌'), "missing monospace c");
}
