//! The text console on the screen: what programs write goes into a grid
//! of characters, which is drawn a changed row at a time in `JetBrains
//! Mono`, light on dark, the cursor a block of sun, each character in its
//! own cell from a glyph rasterized once. Once a crash takes the screen,
//! the console draws no more.

#[cfg(feature = "qemu-test")]
use alloc::string::String;

use canvas::palette::{NIGHT, SUN, TEXT};
use canvas::{Scratch, Style};
use font::Font;
use kernel::console::{Glyph, Glyphs, Grid, cell_pixels, fit};
use kernel::display::Display;

use super::memory::Spin;

/// How big the console's letters are, in pixels to the em.
const PX: f32 = 18.0;

/// The room left free around the grid, in pixels.
const MARGIN: usize = 16;

struct Console {
    display: Display<'static>,
    grid: Grid,
    /// How wide and how high a cell is, and how far its baseline is below
    /// its top, in pixels.
    cell: (usize, usize),
    baseline: usize,
    /// The cell the cursor was last drawn in.
    drawn_cursor: (usize, usize),
}

static CONSOLE: Spin<Option<Console>> = Spin::new(None);

/// The room glyphs are drawn in, too big for a stack, taken while the
/// console draws.
static SCRATCH: Spin<Scratch> = Spin::new(Scratch::new());

/// The glyphs drawn so far.
static GLYPHS: Spin<Glyphs> = Spin::new(Glyphs::new());

fn style() -> Option<Style<'static>> {
    Some(Style {
        font: Font::parse(kernel::panic::MONO).ok()?,
        px: PX,
        color: TEXT,
    })
}

/// Takes `display` over for the console and clears it; `None` when the
/// font could not be read to draw it.
pub fn start(display: Display<'static>) -> Option<()> {
    let style = style()?;
    let metrics = style.font.line_metrics(PX);
    let width = style.font.layout("M", PX, |_| {}).ok()?;
    let (cell_width, cell_height) = (cell_pixels(width), cell_pixels(metrics.line_height()));
    let screen = display.screen();
    let columns = fit(screen.width(), MARGIN, cell_width);
    let rows = fit(screen.height(), MARGIN, cell_height);
    let mut console = Console {
        display,
        grid: Grid::new(columns, rows),
        cell: (cell_width, cell_height),
        baseline: cell_pixels(metrics.ascent),
        drawn_cursor: (0, 0),
    };
    console.display.frame(|c| c.fill(NIGHT));
    draw(&mut console);
    *CONSOLE.lock() = Some(console);
    Some(())
}

/// Writes `text` on the console, when there is one and no crash took the
/// screen.
pub fn write(text: &str) {
    if super::panicking() {
        return;
    }
    if let Some(console) = CONSOLE.lock().as_mut() {
        console.grid.write(text);
        draw(console);
    }
}

/// `c`'s glyph in `style`, rasterized in `scratch`; none for a character
/// that draws nothing or could not be rasterized.
fn glyph(style: &Style<'_>, c: char, scratch: &mut Scratch) -> Option<Glyph> {
    let (p, coverage) = canvas::rasterize(style, c, scratch).ok()??;
    Some(Glyph {
        left: p.left,
        top: p.top,
        width: p.width,
        coverage: coverage.to_vec(),
    })
}

/// Draws the rows that changed and the cursor where it is now. A cursor
/// that waits past the right edge is drawn in the last column, under the
/// character there.
fn draw(console: &mut Console) {
    let Some(style) = style() else {
        return;
    };
    let mut rows = console.grid.changed();
    let cursor = console.grid.cursor();
    for row in [console.drawn_cursor.1, cursor.1] {
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    let (cell_width, cell_height) = console.cell;
    let width = console.grid.columns() * cell_width;
    let at = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    let mut scratch = SCRATCH.lock();
    let mut glyphs = GLYPHS.lock();
    for row in rows {
        let top = MARGIN + row * cell_height;
        let baseline = at(top + console.baseline);
        let grid = &console.grid;
        console
            .display
            .update(at(MARGIN), at(top), width, cell_height, |c| {
                c.fill_rect(at(MARGIN), at(top), width, cell_height, NIGHT);
                if row == cursor.1 {
                    c.fill_rect(
                        at(MARGIN + cursor.0.min(grid.columns() - 1) * cell_width),
                        at(top),
                        cell_width,
                        cell_height,
                        SUN,
                    );
                }
                for (column, &ch) in grid.row(row).iter().enumerate() {
                    if let Some(g) = glyphs.get(ch, |ch| glyph(&style, ch, &mut scratch)) {
                        let pen = at(MARGIN + column * cell_width);
                        c.coverage(pen + g.left, baseline + g.top, g.width, &g.coverage, TEXT);
                    }
                }
            });
    }
    console.drawn_cursor = cursor;
}

/// How many glyphs the console rasterized so far. Only the test image
/// asks.
#[cfg(feature = "qemu-test")]
pub fn rasterized() -> usize {
    GLYPHS.lock().made()
}

/// The characters of the console's row `row`, when there is a console.
/// Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn row(row: usize) -> Option<String> {
    CONSOLE
        .lock()
        .as_ref()
        .map(|c| c.grid.row(row).iter().collect::<String>().trim_end().into())
}

/// How many rows and columns the console has, and where its first cell's
/// pixel is on the screen, with the colour there. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn shape() -> Option<((usize, usize), (usize, usize))> {
    let console = CONSOLE.lock();
    let c = console.as_ref()?;
    Some(((c.grid.columns(), c.grid.rows()), c.cell))
}

/// The colour of the screen's pixel at `x`, `y`, under the console. Only
/// the test image asks.
#[cfg(feature = "qemu-test")]
pub fn pixel(x: usize, y: usize) -> Option<canvas::Color> {
    CONSOLE.lock().as_ref()?.display.screen().pixel(x, y)
}
