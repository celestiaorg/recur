//! Alt-click: the arrow keys that walk the prompt cursor to a clicked cell.

use alacritty_terminal::index::Point;

/// Arrow keys from `from` to `to` (screen points). A shell's line editor wraps, so on the
/// primary screen the cursor walks cell by cell; on the alternate screen it moves by rows, then
/// columns.
pub(crate) fn sequence(
    from: Point,
    to: Point,
    cols: usize,
    alt_screen: bool,
    app_cursor: bool,
) -> String {
    let key = |c: char, n: i64| {
        format!("\x1b{}{c}", if app_cursor { 'O' } else { '[' }).repeat(n.unsigned_abs() as usize)
    };
    let rows = i64::from(to.line.0 - from.line.0);
    let mut cells = to.column.0 as i64 - from.column.0 as i64;
    let mut out = String::new();
    if alt_screen {
        out = key(if rows < 0 { 'A' } else { 'B' }, rows);
    } else {
        cells += rows * cols as i64;
    }
    out + &key(if cells < 0 { 'D' } else { 'C' }, cells)
}
