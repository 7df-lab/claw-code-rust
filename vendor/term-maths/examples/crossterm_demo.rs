//! Demonstrates the crossterm renderer backend.
//!
//! Run with: cargo run --example crossterm_demo --features crossterm
//!
//! This example uses cursor positioning to render the equation at a specific
//! location in the terminal. It must be run in a real terminal (not piped).

#[cfg(feature = "crossterm")]
fn main() -> std::io::Result<()> {
    use crossterm::{cursor, execute, tty::IsTty};
    use std::io::{Write, stdout};
    use term_maths::{CrosstermRenderer, render};

    let block = render(r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}");

    let mut stdout = stdout();

    if !stdout.is_tty() {
        // Fallback: just print via Display when not in a real terminal
        println!("Crossterm renderer demo — quadratic formula:\n");
        println!("{}", block);
        return Ok(());
    }

    println!("Crossterm renderer demo — quadratic formula:\n");

    // Reserve vertical space by printing blank lines, then move back up
    for _ in 0..block.height() {
        println!();
    }

    // Move cursor back to the start of the reserved space
    let (col, row) = cursor::position()?;
    let start_row = row.saturating_sub(block.height() as u16);
    CrosstermRenderer::render_at(&mut stdout, &block, col, start_row)?;

    // Move cursor below the rendered block
    execute!(stdout, cursor::MoveTo(0, row))?;
    stdout.flush()?;
    println!();

    Ok(())
}

#[cfg(not(feature = "crossterm"))]
fn main() {
    eprintln!("This example requires the `crossterm` feature.");
    eprintln!("Run with: cargo run --example crossterm_demo --features crossterm");
}
