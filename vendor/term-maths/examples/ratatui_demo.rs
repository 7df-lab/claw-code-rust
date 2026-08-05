//! Demonstrates the ratatui widget backend.
//!
//! Run with: cargo run --example ratatui_demo --features ratatui

#[cfg(feature = "ratatui")]
fn main() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    use term_maths::{MathWidget, render};

    let block = render(r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}");

    // Create a buffer large enough to hold the rendered block
    let area = Rect::new(0, 0, block.width() as u16 + 2, block.height() as u16 + 1);
    let mut buf = Buffer::empty(area);

    // Render the widget into the buffer
    let widget = MathWidget::new(&block);
    widget.render(area, &mut buf);

    // Print the buffer contents (simulating what ratatui would display)
    println!("Ratatui widget demo — quadratic formula:\n");
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            print!("{}", cell.symbol());
        }
        println!();
    }
}

#[cfg(not(feature = "ratatui"))]
fn main() {
    eprintln!("This example requires the `ratatui` feature.");
    eprintln!("Run with: cargo run --example ratatui_demo --features ratatui");
}
