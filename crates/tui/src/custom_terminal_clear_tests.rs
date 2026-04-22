use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use crate::custom_terminal::Terminal;
use crate::insert_history::insert_history_lines;
use crate::test_backend::VT100Backend;

#[test]
fn clear_managed_inline_area_preserves_rows_above_devo() {
    let width: u16 = 24;
    let height: u16 = 8;
    let backend = VT100Backend::new(width, height);
    let mut term = Terminal::with_options(backend).expect("terminal");
    term.set_viewport_area(Rect::new(0, 2, width, 2));

    insert_history_lines(&mut term, vec!["devo line".into()]).expect("insert history");
    let rows_before: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
    let devo_row = rows_before
        .iter()
        .position(|row| row.contains("devo line"))
        .expect("expected devo line on screen");
    assert_eq!(2, devo_row);

    term.clear_managed_inline_area()
        .expect("clear managed inline area");

    let rows_after: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
    assert_eq!(rows_before[0], rows_after[0]);
    assert_eq!(rows_before[1], rows_after[1]);
    assert_eq!("", rows_after[2].trim_end());
    assert!(
        rows_after.iter().all(|row| !row.contains("devo line")),
        "expected devo-managed rows to be cleared, rows: {rows_after:?}"
    );
}
