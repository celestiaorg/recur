//! xterm.js `moveToCellSequence`: the arrow keys that walk the prompt cursor to a clicked cell
//! (Alt-click). Coordinates are screen-relative, 0-based.

use super::buffer::Buffer;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Right,
    Left,
}

impl Dir {
    fn letter(self) -> char {
        match self {
            Dir::Up => 'A',
            Dir::Down => 'B',
            Dir::Right => 'C',
            Dir::Left => 'D',
        }
    }
}

pub(crate) fn sequence(buf: &Buffer, target_x: i64, target_y: i64, app_cursor: bool) -> String {
    let (start_x, start_y) = buf.cursor();
    let cols = buf.cols() as i64;
    if !buf.has_scrollback() {
        return reset_starting_row(buf, start_x, start_y, target_y, app_cursor)
            + &move_to_requested_row(buf, start_y, target_y, app_cursor)
            + &move_to_requested_col(buf, start_x, start_y, target_x, target_y, app_cursor);
    }
    if start_y == target_y {
        let dir = if start_x > target_x {
            Dir::Left
        } else {
            Dir::Right
        };
        return repeat((start_x - target_x).abs(), &seq(dir, app_cursor));
    }
    let dir = if start_y > target_y {
        Dir::Left
    } else {
        Dir::Right
    };
    let row_diff = (start_y - target_y).abs();
    let from_end = cols
        - if start_y > target_y {
            target_x
        } else {
            start_x
        };
    let from_begin = (if start_y > target_y {
        start_x
    } else {
        target_x
    }) - 1;
    let cells = from_end + (row_diff - 1) * cols + 1 + from_begin;
    repeat(cells, &seq(dir, app_cursor))
}

fn reset_starting_row(
    buf: &Buffer,
    start_x: i64,
    start_y: i64,
    target_y: i64,
    app_cursor: bool,
) -> String {
    if move_to_requested_row(buf, start_y, target_y, app_cursor).is_empty() {
        return String::new();
    }
    let n = buffer_line(
        buf,
        start_x,
        start_y,
        start_x,
        start_y - wrapped_rows_for_row(buf, start_y),
        false,
    )
    .chars()
    .count();
    repeat(n as i64, &seq(Dir::Left, app_cursor))
}

fn move_to_requested_row(buf: &Buffer, start_y: i64, target_y: i64, app_cursor: bool) -> String {
    let start_row = start_y - wrapped_rows_for_row(buf, start_y);
    let end_row = target_y - wrapped_rows_for_row(buf, target_y);
    let rows = (start_row - end_row).abs() - wrapped_rows_count(buf, start_y, target_y);
    repeat(rows, &seq(vertical(start_y, target_y), app_cursor))
}

fn move_to_requested_col(
    buf: &Buffer,
    start_x: i64,
    start_y: i64,
    target_x: i64,
    target_y: i64,
    app_cursor: bool,
) -> String {
    let start_row = if move_to_requested_row(buf, start_y, target_y, app_cursor).is_empty() {
        start_y
    } else {
        target_y - wrapped_rows_for_row(buf, target_y)
    };
    let dir = horizontal(buf, start_x, start_y, target_x, target_y, app_cursor);
    let n = buffer_line(
        buf,
        start_x,
        start_row,
        target_x,
        target_y,
        dir == Dir::Right,
    )
    .chars()
    .count();
    repeat(n as i64, &seq(dir, app_cursor))
}

fn wrapped_rows_count(buf: &Buffer, start_y: i64, target_y: i64) -> i64 {
    let start_row = start_y - wrapped_rows_for_row(buf, start_y);
    let end_row = target_y - wrapped_rows_for_row(buf, target_y);
    let step = if vertical(start_y, target_y) == Dir::Up {
        -1
    } else {
        1
    };
    (0..(start_row - end_row).abs())
        .filter(|i| is_wrapped(buf, start_row + step * i))
        .count() as i64
}

fn wrapped_rows_for_row(buf: &Buffer, mut row: i64) -> i64 {
    let mut count = 0;
    let mut wraps = is_wrapped(buf, row);
    while wraps && row >= 0 && row < buf.rows() as i64 {
        count += 1;
        row -= 1;
        wraps = is_wrapped(buf, row);
    }
    count
}

fn is_wrapped(buf: &Buffer, row: i64) -> bool {
    row >= 0 && buf.is_wrapped(row as usize)
}

fn horizontal(
    buf: &Buffer,
    start_x: i64,
    start_y: i64,
    target_x: i64,
    target_y: i64,
    app_cursor: bool,
) -> Dir {
    let start_row = if move_to_requested_row(buf, start_y, target_y, app_cursor).is_empty() {
        start_y
    } else {
        target_y - wrapped_rows_for_row(buf, target_y)
    };
    if (start_x < target_x && start_row <= target_y)
        || (start_x >= target_x && start_row < target_y)
    {
        Dir::Right
    } else {
        Dir::Left
    }
}

fn vertical(start_y: i64, target_y: i64) -> Dir {
    if start_y > target_y {
        Dir::Up
    } else {
        Dir::Down
    }
}

fn buffer_line(
    buf: &Buffer,
    mut start_col: i64,
    start_row: i64,
    end_col: i64,
    end_row: i64,
    forward: bool,
) -> String {
    let cols = buf.cols() as i64;
    let mut cur_col = start_col;
    let mut cur_row = start_row;
    let mut out = String::new();
    let tr = |row: i64, s: i64, e: i64| {
        if row < 0 {
            return String::new();
        }
        buf.translate(
            row as usize,
            false,
            s.max(0) as usize,
            Some(e.max(0) as usize),
        )
    };
    while (cur_col != end_col || cur_row != end_row) && cur_row >= 0 && cur_row < buf.len() as i64 {
        cur_col += if forward { 1 } else { -1 };
        if forward && cur_col > cols - 1 {
            out += &tr(cur_row, start_col, cur_col);
            cur_col = 0;
            start_col = 0;
            cur_row += 1;
        } else if !forward && cur_col < 0 {
            out += &tr(cur_row, 0, start_col + 1);
            cur_col = cols - 1;
            start_col = cur_col;
            cur_row -= 1;
        }
    }
    out + &tr(cur_row, start_col, cur_col)
}

fn seq(dir: Dir, app_cursor: bool) -> String {
    format!("\x1b{}{}", if app_cursor { 'O' } else { '[' }, dir.letter())
}

fn repeat(count: i64, s: &str) -> String {
    s.repeat(count.max(0) as usize)
}
