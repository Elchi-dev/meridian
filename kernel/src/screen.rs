//! The status screen: what the kernel knows about the machine, drawn in
//! the boot screens' fixed dark look.

use bootinfo::{MemoryRegion, RegionKind};
use canvas::palette::{BORDER, GRASS, MUTED, NIGHT, SKY, SURFACE, TANGERINE, TEAL, TEXT, TOMATO};
use canvas::{Canvas, Color, Scratch, Style};
use font::{Error, Font};

use crate::{Size, bytes_of};

static INTER: &[u8] = include_bytes!("../../third-party/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../third-party/fonts/Inter-SemiBold.ttf");

/// What the screen shows.
pub struct Facts<'a> {
    pub processor: &'a str,
    pub regions: &'a [MemoryRegion],
}

const MARGIN: i32 = 56;
const LABEL_WIDTH: i32 = 140;
const BAR_HEIGHT: usize = 28;

/// The colour each kind of memory gets on the bar and in its legend.
#[must_use]
pub fn color_of(kind: RegionKind) -> Color {
    match kind {
        RegionKind::USABLE => GRASS,
        RegionKind::KERNEL => TANGERINE,
        RegionKind::ACPI_RECLAIMABLE => SKY,
        RegionKind::ACPI_NVS => TEAL,
        RegionKind::UNUSABLE => TOMATO,
        _ => BORDER,
    }
}

/// Splits `width` pixels between the regions by their size, in address
/// order, and hands each piece to `piece` as its first pixel, its end and
/// its kind. The pieces cover the width exactly; a region too small for a
/// pixel of its own gets none.
pub fn memory_bar(
    regions: &[MemoryRegion],
    width: usize,
    mut piece: impl FnMut(usize, usize, RegionKind),
) {
    let total: u128 = regions.iter().map(|r| u128::from(r.pages)).sum();
    if total == 0 {
        return;
    }
    let mut pages_before = 0u128;
    let mut x = 0;
    for r in regions {
        pages_before += u128::from(r.pages);
        let end = usize::try_from(pages_before * width as u128 / total).unwrap_or(width);
        if end > x {
            piece(x, end, r.kind);
            x = end;
        }
    }
}

/// How much memory is free, out of all memory.
fn memory_line(regions: &[MemoryRegion]) -> Line {
    let free = bytes_of(regions, RegionKind::USABLE);
    let all: u64 = regions
        .iter()
        .map(|r| r.pages * MemoryRegion::PAGE_SIZE)
        .sum();
    let mut line = Line::new();
    let _ = core::fmt::write(
        &mut line,
        format_args!("{} free of {}", Size(free), Size(all)),
    );
    line
}

/// Draws the whole screen.
///
/// # Errors
///
/// The font's errors, which only a broken font file causes.
pub fn draw(
    canvas: &mut Canvas<'_>,
    facts: &Facts<'_>,
    scratch: &mut Scratch,
) -> Result<(), Error> {
    let regular = Font::parse(INTER)?;
    let title = Style {
        font: Font::parse(INTER_SEMIBOLD)?,
        px: 40.0,
        color: TEXT,
    };
    let label = Style {
        font: regular,
        px: 18.0,
        color: MUTED,
    };
    let value = Style {
        color: TEXT,
        ..label
    };

    canvas.fill(NIGHT);
    canvas.text(&title, MARGIN, MARGIN + 40, "Meridian", scratch)?;
    canvas.fill_rect(MARGIN, MARGIN + 58, 64, 6, TANGERINE);

    let regions = crate::memory(facts.regions);
    let memory = memory_line(regions);
    let mut kernel = Line::new();
    let _ = core::fmt::write(&mut kernel, format_args!("{}", crate::VERSION));
    let rows = [
        ("Kernel", kernel.as_str()),
        ("Processor", facts.processor),
        ("Memory", memory.as_str()),
    ];
    let mut baseline = MARGIN + 130;
    for (name, text) in rows {
        canvas.text(&label, MARGIN, baseline, name, scratch)?;
        canvas.text(&value, MARGIN + LABEL_WIDTH, baseline, text, scratch)?;
        baseline += 32;
    }

    let bar_width = canvas.width().saturating_sub(2 * MARGIN as usize).min(720);
    let bar_top = baseline + 8;
    canvas.fill_rect(
        MARGIN - 2,
        bar_top - 2,
        bar_width + 4,
        BAR_HEIGHT + 4,
        SURFACE,
    );
    memory_bar(regions, bar_width, |x, end, kind| {
        canvas.fill_rect(
            MARGIN + to_i32(x),
            bar_top,
            end - x,
            BAR_HEIGHT,
            color_of(kind),
        );
    });

    let legend = [
        ("free", RegionKind::USABLE),
        ("kernel", RegionKind::KERNEL),
        ("ACPI", RegionKind::ACPI_RECLAIMABLE),
        ("firmware", RegionKind::RESERVED),
    ];
    let mut x = MARGIN;
    let legend_baseline = bar_top + to_i32(BAR_HEIGHT) + 32;
    for (name, kind) in legend {
        canvas.fill_rect(x, legend_baseline - 12, 12, 12, color_of(kind));
        let width = canvas.text(&label, x + 20, legend_baseline, name, scratch)?;
        x += 20 + i32::try_from(width).unwrap_or(i32::MAX / 2) + 24;
    }
    Ok(())
}

/// A screen position from a size, which on a screen is always small.
fn to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX / 2)
}

/// A line of text built without a heap.
struct Line {
    bytes: [u8; 96],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line {
            bytes: [0; 96],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = self.bytes.len() - self.len;
        if s.len() > room {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Format;
    use core::fmt::Write;

    fn region(start: u64, pages: u64, kind: RegionKind) -> MemoryRegion {
        MemoryRegion {
            start,
            pages,
            kind,
            reserved: 0,
        }
    }

    fn pieces(regions: &[MemoryRegion], width: usize) -> Vec<(usize, usize, RegionKind)> {
        let mut out = Vec::new();
        memory_bar(regions, width, |x, end, kind| out.push((x, end, kind)));
        out
    }

    fn sample() -> Vec<MemoryRegion> {
        vec![
            region(0, 0x9f, RegionKind::USABLE),
            region(0x10_0000, 0x40, RegionKind::KERNEL),
            region(0x14_0000, 0x1_7fc0, RegionKind::USABLE),
            region(0x1800_0000, 0x10, RegionKind::ACPI_RECLAIMABLE),
            region(0xffc0_0000, 0x400, RegionKind::RESERVED),
        ]
    }

    #[test]
    fn the_bar_covers_its_width_in_address_order() {
        let got = pieces(&sample(), 700);
        assert_eq!(got.first().map(|p| p.0), Some(0));
        assert_eq!(got.last().map(|p| p.1), Some(700));
        for pair in got.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "no gaps, no overlap");
        }
        let kinds: Vec<_> = got.iter().map(|p| p.2).collect();
        assert_eq!(
            kinds,
            [RegionKind::USABLE, RegionKind::USABLE, RegionKind::RESERVED]
        );
    }

    #[test]
    fn pieces_follow_the_sizes() {
        let half = [
            region(0, 50, RegionKind::USABLE),
            region(0x10_0000, 50, RegionKind::KERNEL),
        ];
        assert_eq!(
            pieces(&half, 100),
            [(0, 50, RegionKind::USABLE), (50, 100, RegionKind::KERNEL)]
        );
        let thirds = [
            region(0, 1, RegionKind::USABLE),
            region(0x1000, 1, RegionKind::KERNEL),
            region(0x2000, 1, RegionKind::RESERVED),
        ];
        assert_eq!(
            pieces(&thirds, 10),
            [
                (0, 3, RegionKind::USABLE),
                (3, 6, RegionKind::KERNEL),
                (6, 10, RegionKind::RESERVED)
            ]
        );
    }

    #[test]
    fn an_empty_map_draws_nothing() {
        assert_eq!(pieces(&[], 100), []);
        assert_eq!(pieces(&[region(0, 0, RegionKind::USABLE)], 100), []);
        assert_eq!(pieces(&sample(), 0), []);
    }

    #[test]
    fn huge_maps_do_not_overflow() {
        let big = [
            region(0, u64::MAX / 4096, RegionKind::USABLE),
            region(0, u64::MAX / 4096, RegionKind::RESERVED),
        ];
        assert_eq!(
            pieces(&big, 10),
            [(0, 5, RegionKind::USABLE), (5, 10, RegionKind::RESERVED)]
        );
    }

    #[test]
    fn every_kind_has_its_colour() {
        let kinds = [
            (RegionKind::USABLE, GRASS),
            (RegionKind::KERNEL, TANGERINE),
            (RegionKind::ACPI_RECLAIMABLE, SKY),
            (RegionKind::ACPI_NVS, TEAL),
            (RegionKind::UNUSABLE, TOMATO),
            (RegionKind::RESERVED, BORDER),
            (RegionKind(99), BORDER),
        ];
        for (kind, color) in kinds {
            assert_eq!(color_of(kind), color, "{kind:?}");
        }
    }

    #[test]
    fn lines_refuse_to_overflow() {
        let mut line = Line::new();
        assert!(write!(line, "{}", "x".repeat(96)).is_ok());
        assert_eq!(line.as_str().len(), 96);
        assert!(write!(line, "y").is_err());
        let mut line = Line::new();
        assert!(write!(line, "{}", "z".repeat(97)).is_err());
        assert_eq!(line.as_str(), "");
    }

    fn drawn(width: usize, height: usize) -> Vec<u32> {
        let mut pixels = vec![0u32; width * height];
        let mut canvas = Canvas::new(&mut pixels, width, height, width, Format::Bgr).unwrap();
        let facts = Facts {
            processor: "Test CPU 9000",
            regions: &sample(),
        };
        draw(&mut canvas, &facts, &mut Scratch::new()).unwrap();
        pixels
    }

    fn color_at(pixels: &mut [u32], width: usize, height: usize, x: usize, y: usize) -> Color {
        Canvas::new(pixels, width, height, width, Format::Bgr)
            .unwrap()
            .pixel(x, y)
            .unwrap()
    }

    #[test]
    fn the_screen_has_its_title_bar_and_legend() {
        let (w, h) = (1024, 600);
        let mut px = drawn(w, h);
        let mut at = |x: usize, y: usize| color_at(&mut px, w, h, x, y);
        assert_eq!(at(0, 0), NIGHT);
        assert_eq!(at(w - 1, h - 1), NIGHT);
        assert_eq!(at(60, 116), TANGERINE, "the accent under the title");
        let title_ink = (60..260)
            .flat_map(|x| (60..100).map(move |y| (x, y)))
            .filter(|&(x, y)| at(x, y) != NIGHT)
            .count();
        assert!(title_ink > 400, "{title_ink}");

        // Three rows from MARGIN + 130, 32 apart, then the bar 8 below.
        let bar_top = 56 + 130 + 3 * 32 + 8;
        assert_eq!(
            at(56, bar_top + 1),
            GRASS,
            "the bar starts with free memory"
        );
        assert_eq!(
            at(56 + 719, bar_top + 1),
            SKY,
            "and ends with the ACPI tables; the firmware above memory is left out"
        );
        assert_eq!(at(56 + 720, bar_top + 1), SURFACE, "the frame around it");
        assert_eq!(at(56 + 721, bar_top + 1), SURFACE, "two pixels wide");
        assert_eq!(at(56 + 722, bar_top + 1), NIGHT);
        let legend = bar_top + 28 + 32 - 6;
        assert_eq!(at(60, legend), GRASS);
    }

    #[test]
    fn the_memory_line_counts_memory_only() {
        // 0x9f + 0x1_7fc0 pages free, 0x40 kernel and 0x10 ACPI pages, and
        // 0x400 firmware pages above memory.
        assert_eq!(
            memory_line(&sample()).as_str(),
            "384.3 MiB free of 388.6 MiB"
        );
        assert_eq!(
            memory_line(crate::memory(&sample())).as_str(),
            "384.3 MiB free of 384.6 MiB",
            "without the firmware above memory"
        );
        assert_eq!(memory_line(&[]).as_str(), "0 bytes free of 0 bytes");
    }

    #[test]
    fn drawing_is_repeatable_and_the_same_in_both_pixel_orders() {
        let (w, h) = (640, 480);
        let facts = Facts {
            processor: "Same",
            regions: &sample(),
        };
        let mut pictures = Vec::new();
        for format in [Format::Bgr, Format::Bgr, Format::Rgb] {
            let mut px = vec![0u32; w * h];
            let mut canvas = Canvas::new(&mut px, w, h, w, format).unwrap();
            draw(&mut canvas, &facts, &mut Scratch::new()).unwrap();
            let colors: Vec<Color> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| canvas.pixel(x, y).unwrap())
                .collect();
            pictures.push((px, colors));
        }
        assert_eq!(pictures[0].0, pictures[1].0, "the same pixels twice");
        assert_eq!(
            pictures[0].1, pictures[2].1,
            "the same colours in either order"
        );
        assert_ne!(pictures[0].0, pictures[2].0);
    }

    #[test]
    fn small_screens_and_empty_facts_still_draw() {
        for (w, h) in [(1, 1), (120, 80), (320, 200)] {
            let mut px = vec![0u32; w * h];
            let mut canvas = Canvas::new(&mut px, w, h, w, Format::Bgr).unwrap();
            let facts = Facts {
                processor: "",
                regions: &[],
            };
            draw(&mut canvas, &facts, &mut Scratch::new()).unwrap();
            assert_eq!(canvas.pixel(0, 0), Some(NIGHT), "{w} by {h}");
        }
    }

    #[test]
    fn the_legend_names_each_colour_in_order() {
        let (w, h) = (1024, 600);
        let mut px = drawn(w, h);
        let row = 56 + 130 + 3 * 32 + 8 + 28 + 32 - 6;
        let mut found = Vec::new();
        for x in 0..w {
            let c = color_at(&mut px, w, h, x, row);
            if [GRASS, TANGERINE, SKY, BORDER].contains(&c)
                && found.last().is_none_or(|&(_, last)| last != c)
            {
                found.push((x, c));
            }
        }
        let colors: Vec<Color> = found.iter().map(|f| f.1).collect();
        assert_eq!(colors, [GRASS, TANGERINE, SKY, BORDER]);
        assert_eq!(found[0].0, 56);
        assert!(found.windows(2).all(|p| p[1].0 > p[0].0 + 40), "{found:?}");
    }

    #[test]
    fn labels_are_muted_and_values_bright() {
        let (w, h) = (1024, 600);
        let mut px = drawn(w, h);
        let brightest = |px: &mut [u32], xs: core::ops::Range<usize>, y0: usize| {
            let mut top = 0u8;
            for y in y0 - 16..y0 + 4 {
                for x in xs.clone() {
                    top = top.max(color_at(px, w, h, x, y).g);
                }
            }
            top
        };
        let baseline = 56 + 130;
        assert_eq!(brightest(&mut px, 56..180, baseline), MUTED.g, "the label");
        assert_eq!(brightest(&mut px, 196..400, baseline), TEXT.g, "the value");
        assert_eq!(brightest(&mut px, 56..260, 56 + 40), TEXT.g, "the title");
    }

    #[test]
    fn a_narrow_screen_shrinks_the_bar() {
        let (w, h) = (400, 500);
        let mut px = drawn(w, h);
        let bar_top = 56 + 130 + 3 * 32 + 8;
        assert_eq!(color_at(&mut px, w, h, 56 + 287, bar_top + 1), SKY);
        assert_eq!(color_at(&mut px, w, h, 56 + 288, bar_top + 1), SURFACE);
    }
}
