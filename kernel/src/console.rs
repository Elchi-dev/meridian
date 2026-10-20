//! The text console, the parts that need no screen: rows of characters, a
//! cursor that text moves on, lines that wrap at the right edge, a screen
//! that scrolls up once text runs past its last row, and which rows have
//! changed since they were last drawn. The kernel draws the rows; this
//! says what is in them.

use alloc::vec;
use alloc::vec::Vec;

/// Where a tab stops: at every eighth column.
const TAB: usize = 8;

/// `measure`, a length a font gives, in whole pixels rounded up, so a
/// cell holds every glyph; at most 4096.
#[must_use]
pub fn cell_pixels(measure: f32) -> usize {
    let mut whole: u16 = 0;
    while f32::from(whole) < measure && whole < 4096 {
        whole += 1;
    }
    usize::from(whole)
}

/// How many cells of `cell` pixels fit across `screen` pixels with
/// `margin` left free on either side; one at least.
#[must_use]
pub fn fit(screen: usize, margin: usize, cell: usize) -> usize {
    (screen.saturating_sub(2 * margin) / cell.max(1)).max(1)
}

/// What the console shows: `columns` characters in each of `rows` rows.
pub struct Grid {
    columns: usize,
    rows: usize,
    cells: Vec<char>,
    column: usize,
    row: usize,
    changed: Vec<bool>,
}

impl Grid {
    /// An empty console of `columns` by `rows`, at least one of each, with
    /// every row still to be drawn.
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> Grid {
        let (columns, rows) = (columns.max(1), rows.max(1));
        Grid {
            columns,
            rows,
            cells: vec![' '; columns * rows],
            column: 0,
            row: 0,
            changed: vec![true; rows],
        }
    }

    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
    }

    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Where the next character goes: its column and its row.
    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        (self.column, self.row)
    }

    /// The characters of row `row`, spaces where there are none.
    #[must_use]
    pub fn row(&self, row: usize) -> &[char] {
        let start = row.min(self.rows - 1) * self.columns;
        &self.cells[start..start + self.columns]
    }

    /// Writes `text` from the cursor on: a new line goes to the start of
    /// the next row, a carriage return to the start of this one, a tab to
    /// the next eighth column, a backspace back one column, clearing it,
    /// and a form feed clears the console. Other control characters are
    /// left out. A line that reaches the
    /// right edge goes on in the next row, and text past the last row
    /// moves every row up one.
    pub fn write(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '\n' => self.next_line(),
                '\r' => self.column = 0,
                '\u{c}' => self.clear(),
                '\t' => {
                    let stop = (self.column / TAB + 1) * TAB;
                    while self.column < stop.min(self.columns) {
                        self.put(' ');
                    }
                }
                '\u{8}' => {
                    if self.column > 0 {
                        self.column -= 1;
                        self.set(self.column, self.row, ' ');
                    }
                }
                c if c.is_control() => {}
                c => self.put(c),
            }
        }
    }

    /// Clears every row and puts the cursor at the top left.
    pub fn clear(&mut self) {
        self.cells.fill(' ');
        self.changed.fill(true);
        self.column = 0;
        self.row = 0;
    }

    /// The rows that changed since this was last asked, each once, from
    /// the top down.
    pub fn changed(&mut self) -> Vec<usize> {
        let rows = (0..self.rows).filter(|&r| self.changed[r]).collect();
        self.changed.fill(false);
        rows
    }

    fn put(&mut self, c: char) {
        if self.column == self.columns {
            self.next_line();
        }
        self.set(self.column, self.row, c);
        self.column += 1;
    }

    fn set(&mut self, column: usize, row: usize, c: char) {
        self.cells[row * self.columns + column] = c;
        self.changed[row] = true;
    }

    fn next_line(&mut self) {
        self.column = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        self.cells.copy_within(self.columns.., 0);
        let last = (self.rows - 1) * self.columns;
        self.cells[last..].fill(' ');
        self.changed.fill(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(g: &Grid, row: usize) -> String {
        g.row(row).iter().collect::<String>().trim_end().to_string()
    }

    #[test]
    fn text_goes_where_the_cursor_is() {
        let mut g = Grid::new(10, 3);
        g.write("hello");
        assert_eq!((text(&g, 0), g.cursor()), ("hello".to_string(), (5, 0)));
        g.write("\nworld");
        assert_eq!((text(&g, 1), g.cursor()), ("world".to_string(), (5, 1)));
        g.write("\rW");
        assert_eq!(text(&g, 1), "World", "back to the start of the row");
        assert_eq!(g.row(2), [' '; 10]);
        assert_eq!((g.columns(), g.rows()), (10, 3));
    }

    #[test]
    fn a_line_wraps_at_the_right_edge() {
        let mut g = Grid::new(4, 3);
        g.write("abcd");
        assert_eq!(g.cursor(), (4, 0), "it waits at the edge");
        g.write("e");
        assert_eq!(
            (text(&g, 0), text(&g, 1), g.cursor()),
            ("abcd".to_string(), "e".to_string(), (1, 1))
        );
        let mut exact = Grid::new(4, 3);
        exact.write("abcd\nx");
        assert_eq!(
            (text(&exact, 1), text(&exact, 2)),
            ("x".to_string(), String::new()),
            "no empty row after a full one"
        );
    }

    #[test]
    fn text_past_the_last_row_moves_every_row_up() {
        let mut g = Grid::new(5, 2);
        g.write("one\ntwo\nthree");
        assert_eq!(
            (text(&g, 0), text(&g, 1)),
            ("two".to_string(), "three".to_string())
        );
        assert_eq!(g.cursor(), (5, 1));
        g.write("!");
        assert_eq!(
            (text(&g, 0), text(&g, 1)),
            ("three".to_string(), "!".to_string()),
            "wrapping scrolls too"
        );
    }

    #[test]
    fn a_tab_goes_to_the_next_eighth_column() {
        let mut g = Grid::new(20, 1);
        g.write("a\tb");
        assert_eq!(g.row(0)[8], 'b');
        g.write("\tc");
        assert_eq!(g.row(0)[16], 'c');
        let mut narrow = Grid::new(10, 2);
        narrow.write("abcdefghi\tx");
        assert_eq!(
            (text(&narrow, 0), text(&narrow, 1)),
            ("abcdefghi".to_string(), "x".to_string()),
            "a tab stops at the edge"
        );
    }

    #[test]
    fn backspace_takes_the_last_character_back() {
        let mut g = Grid::new(10, 2);
        g.write("abc\u{8}\u{8}x");
        assert_eq!((text(&g, 0), g.cursor()), ("ax".to_string(), (2, 0)));
        g.write("\u{8}\u{8}\u{8}");
        assert_eq!(
            (text(&g, 0), g.cursor()),
            (String::new(), (0, 0)),
            "not past the start of the row"
        );
        let mut full = Grid::new(3, 2);
        full.write("abc\u{8}");
        assert_eq!(text(&full, 0), "ab", "at the edge it takes the last one");
    }

    #[test]
    fn control_characters_show_nothing() {
        let mut g = Grid::new(10, 1);
        g.write("a\u{1b}[31mb\u{7}c");
        assert_eq!(
            text(&g, 0),
            "a[31mbc",
            "an escape is left out, what follows it is text"
        );
        g.write("\u{e4}\u{20ac}");
        assert_eq!(g.row(0)[7], '\u{e4}', "letters beyond ASCII are characters");
    }

    #[test]
    fn rows_are_drawn_again_only_once_they_changed() {
        let mut g = Grid::new(5, 3);
        assert_eq!(g.changed(), [0, 1, 2], "a new console is drawn whole");
        assert_eq!(g.changed(), Vec::<usize>::new());
        g.write("x\ny");
        assert_eq!(g.changed(), [0, 1]);
        g.write("\n\nz");
        assert_eq!(g.changed(), [0, 1, 2], "scrolling moves every row");
        g.clear();
        assert_eq!((g.changed(), g.cursor()), (vec![0, 1, 2], (0, 0)));
        assert_eq!(g.row(0), [' '; 5]);
        g.write("ab\ncd\u{c}e");
        assert_eq!(
            (text(&g, 0), text(&g, 1), g.cursor()),
            ("e".to_string(), String::new(), (1, 0)),
            "a form feed clears it"
        );
    }

    #[test]
    fn a_long_text_wraps_and_scrolls_in_one_go() {
        let mut g = Grid::new(4, 3);
        g.write("abcdefghijklmn");
        assert_eq!(
            (text(&g, 0), text(&g, 1), text(&g, 2)),
            ("efgh".to_string(), "ijkl".to_string(), "mn".to_string()),
            "four rows of it in three, the first gone"
        );
        assert_eq!(g.cursor(), (2, 2));
        g.write("\n\n");
        assert_eq!(
            (text(&g, 0), text(&g, 2), g.cursor()),
            ("mn".to_string(), String::new(), (0, 2))
        );
    }

    #[test]
    fn only_the_rows_written_in_change() {
        let mut g = Grid::new(6, 4);
        let _ = g.changed();
        g.write("\n\nab");
        assert_eq!(g.changed(), [2], "moving down changes no row");
        g.write("\u{8}");
        assert_eq!(g.changed(), [2]);
        g.write("\r");
        assert_eq!(
            g.changed(),
            Vec::<usize>::new(),
            "nor does going back to the start"
        );
        g.write("\u{8}");
        assert_eq!(
            g.changed(),
            Vec::<usize>::new(),
            "a backspace at the start changes nothing"
        );
    }

    #[test]
    fn as_many_cells_fit_as_the_screen_has_room_for() {
        assert_eq!(fit(1280, 16, 11), 113, "1248 pixels of cells 11 wide");
        assert_eq!(fit(800, 16, 24), 32);
        assert_eq!(fit(1286, 16, 11), 114, "a whole cell more");
        assert_eq!(fit(1285, 16, 11), 113, "a pixel short of one");
        assert_eq!(fit(20, 16, 11), 1, "no room: one cell anyway");
        assert_eq!(fit(100, 0, 0), 100, "a cell is a pixel at least");
    }

    #[test]
    fn a_cell_takes_its_glyphs_whole_pixels() {
        assert_eq!(cell_pixels(10.8), 11);
        assert_eq!(cell_pixels(11.0), 11, "a whole measure stays");
        assert_eq!(cell_pixels(0.1), 1);
        assert_eq!(cell_pixels(0.0), 0);
        assert_eq!(cell_pixels(-3.0), 0);
        assert_eq!(cell_pixels(1.0e9), 4096, "no cell larger than a screen");
    }

    #[test]
    fn a_console_has_room_for_one_character_at_least() {
        let mut g = Grid::new(0, 0);
        assert_eq!((g.columns(), g.rows()), (1, 1));
        g.write("ab");
        assert_eq!(text(&g, 0), "b");
        assert_eq!(g.row(9), ['b'], "a row past the last is the last");
    }
}
