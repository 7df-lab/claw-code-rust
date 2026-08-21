# term-maths

Character-grid mathematical notation renderer for terminals, implemented in Rust.

Accepts LaTeX math input and renders it as 2D Unicode character art in a terminal. Targets [JuliaMono](https://juliamono.netlify.app/) as the recommended font for full Unicode math symbol coverage.

Available as both a **Rust crate** and a **Python package** (via PyO3 + Maturin).

## Rust Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
term-maths = "0.1"
```

Render a LaTeX expression:

```rust
let block = term_maths::render(r"\frac{a}{b}");
println!("{}", block);
```

Output:

```text
 a
───
 b
```

## Python Usage

Requires a Rust toolchain and [maturin](https://www.maturin.rs):

```sh
pip install maturin
maturin develop --features python   # from the repo root
```

```python
import term_maths

block = term_maths.render(r"\frac{a}{b}")
print(block)
#  a
# ───
#  b

# Compose blocks side-by-side (baseline-aligned)
lhs = term_maths.render(r"\frac{a}{b}")
rhs = term_maths.render(r"\frac{c}{d}")
sep = term_maths.RenderedBlock.from_text(" = ")
print(lhs.beside(sep).beside(rhs))

# Unicode math fonts
print(term_maths.map_str("blackboard", "NZQRC"))  # ℕℤℚℝℂ

# LaTeX round-trip
print(term_maths.to_latex(r"x^2 + y^2"))
```

See `python/examples/` for more: `render_demo.py`, `dsp_equations.py`,
`block_composition.py`, and `math_fonts.py`.

## Rendering Examples

All output below is produced directly by the library.

### Fractions and Arithmetic

```text
--- \frac{a}{b} ---

 a
───
 b

--- \frac{1}{1+\frac{1}{x}} ---

    1
─────────
      1
 1 + ───
      x

--- \frac{-b \pm \sqrt{b^2 - 4ac}}{2a} ---

       ────────
 -b ± √b² - 4ac
────────────────
       2a
```

### Superscripts, Subscripts, and Inline Unicode

```text
x^2          →  x²
a_n          →  aₙ
x_i^2        →  x²ᵢ
a + b = c    →  a + b = c
x^2 + y^2    →  x² + y² = z²
```

### Big Operators with Limits

```text
--- \sum_{n=0}^{N-1} ---

N - 1
  ∑
n = 0

--- \int_{0}^{1} ---

1
⌠
⎮
⌡
0

--- \prod_{i=1}^{n} ---

  n
  ∏
i = 1
```

### DSP Reference Equations

```text
--- DFT Summation: X[k] = \sum_{n=0}^{N-1} x[n] \cdot e^{-j \frac{2\pi}{N} kn} ---

                        2π
                    -j ──── kn
       N - 1            N
X[k] =   ∑   x[n] ·e
       n = 0

--- Convolution Integral ---

             ∞
             ⌠
(f · g)(t) = ⎮  f(τ) g(t - τ) dτ
             ⌡
             -∞

--- Transfer Function ---

        b₀ + b₁ z⁻¹ + b₂ z⁻²
H(z) = ──────────────────────
        1 + a₁ z⁻¹ + a₂ z⁻²

--- Hann Window ---

           ⎛       ⎛  2πn  ⎞⎞
w(n) = 0.5 ⎜1 - cos⎜───────⎟⎟
           ⎝       ⎝ N - 1 ⎠⎠
```

### Matrices

```text
--- pmatrix ---    --- bmatrix ---    --- vmatrix ---

⎛a  b⎞             ⎡1  0⎤             │a  b│
⎝c  d⎠             ⎣0  1⎦             │c  d│

--- 3x3 bmatrix ---

⎡1  2  3⎤
⎢4  5  6⎥
⎣7  8  9⎦

--- Matrix with fractions ---

⎛ 1      ⎞
⎜───   0 ⎟
⎜ 2      ⎟
⎜      3 ⎟
⎜ 0   ───⎟
⎝      4 ⎠
```

### Delimiters, Sqrt, and Accents

```text
--- \left(\frac{a}{b}\right) ---

⎛ a ⎞
⎜───⎟
⎝ b ⎠

--- \sqrt{\frac{a}{b}} ---

 ───
│ a
│───
√ b

--- \overline{x + y} ---

‾‾‾‾‾
x + y
```

### Math Fonts (Unicode Mathematical Alphanumeric Symbols)

```text
\mathbb{R}      →  ℝ
\mathbb{Z}      →  ℤ
\mathcal{L}     →  ℒ
\mathbf{x}      →  𝐱
\mathfrak{g}    →  𝔤
\mathbb{R}^n    →  ℝⁿ
```

## Output Backends

**Core** (always available):

- Plain text via `render()` and `Display`
- LaTeX round-trip via `to_latex()`

**Optional** (feature-gated):

```toml
[dependencies]
term-maths = { version = "0.1", features = ["crossterm", "ratatui"] }
```

| Feature     | Backend             | Description                                            |
|-------------|---------------------|--------------------------------------------------------|
| `crossterm` | `CrosstermRenderer` | Direct terminal output with cursor positioning         |
| `ratatui`   | `MathWidget`        | TUI widget implementing `ratatui::Widget`              |
| `python`    | PyO3 extension      | Python bindings (`maturin build --features python`)    |

### Crossterm

```rust
use term_maths::{render, CrosstermRenderer};

let block = render(r"\sum_{i=0}^{n} x_i");
CrosstermRenderer::print_at(&block, 0, 0)?;
```

### Ratatui

```rust
use term_maths::{render, MathWidget};

let block = render(r"\frac{a}{b}");
let widget = MathWidget::new(&block);
widget.render(area, buf);
```

### LaTeX Round-Trip

```rust
let latex = term_maths::to_latex(r"x^2 + y^2");
// "x^{2} \;+\; y^{2}"
```

## Font Recommendation

For best results, use [JuliaMono](https://juliamono.netlify.app/). It provides complete coverage of:

- Mathematical Alphanumeric Symbols (U+1D400-U+1D7FF) for bold, italic, script, fraktur, double-struck, sans-serif variants
- Box-drawing and bracket piece characters for delimiters and integrals
- Full Greek alphabet and mathematical operators
- Superscript/subscript digits and letters

Other monospace fonts will work but may show fallback glyphs for some mathematical symbols.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
