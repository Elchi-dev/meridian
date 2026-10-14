//! The boot screen: one card per system in the boot screens' fixed dark
//! look, solid and without gradients, and the countdown below them.

use canvas::palette::{BORDER, GRASS, INK, MUTED, NIGHT, SKY, SURFACE, TANGERINE, TEAL, TEXT};
use canvas::{Canvas, Color, Scratch, Style};
use font::{Error, Font};

use crate::menu::Menu;

static INTER: &[u8] = include_bytes!("../../third-party/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../third-party/fonts/Inter-SemiBold.ttf");

/// What kind of system a card stands for, which picks its colour and the
/// letter on its tile. Other systems' logos are not drawn, only letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Meridian,
    Windows,
    Linux,
    Other,
}

impl Kind {
    #[must_use]
    pub fn color(self) -> Color {
        match self {
            Kind::Meridian => TANGERINE,
            Kind::Windows => SKY,
            Kind::Linux => GRASS,
            Kind::Other => TEAL,
        }
    }
}

/// One system on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card<'a> {
    pub kind: Kind,
    pub name: &'a str,
}

pub const CARD_WIDTH: usize = 200;
pub const CARD_HEIGHT: usize = 230;
pub const GAP: usize = 32;
pub const TILE: usize = 96;
/// How far the solid shadow sits below and right of each card.
pub const SHADOW: usize = 6;
const SHADOW_COLOR: Color = Color::hex(0x000b_0c0e);

/// Where the cards start and how many fit side by side.
#[must_use]
pub fn cards_layout(width: usize, height: usize, count: usize) -> (usize, usize, usize) {
    let fits = (width.saturating_sub(2 * GAP) + GAP) / (CARD_WIDTH + GAP);
    let shown = count.min(fits.max(1));
    let row = shown * CARD_WIDTH + shown.saturating_sub(1) * GAP;
    (
        width.saturating_sub(row) / 2,
        height.saturating_sub(CARD_HEIGHT) / 2,
        shown,
    )
}

fn at(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX / 2)
}

/// The width of `text` in `style`, rounded to whole pixels.
fn width_of(style: &Style<'_>, text: &str) -> Result<usize, Error> {
    let w = style.font.layout(text, style.px, |_| {})?;
    // Text on a screen is far narrower than where the cast would cut.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(w as usize)
}

fn centred(
    canvas: &mut Canvas<'_>,
    style: &Style<'_>,
    centre: usize,
    baseline: usize,
    text: &str,
    scratch: &mut Scratch,
) -> Result<(), Error> {
    let w = width_of(style, text)?;
    canvas.text(
        style,
        at(centre.saturating_sub(w / 2)),
        at(baseline),
        text,
        scratch,
    )?;
    Ok(())
}

/// A frame `thickness` pixels wide around a rectangle.
fn frame(
    canvas: &mut Canvas<'_>,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    thickness: usize,
    color: Color,
) {
    canvas.fill_rect(at(x), at(y), w, thickness, color);
    canvas.fill_rect(at(x), at(y + h - thickness), w, thickness, color);
    canvas.fill_rect(at(x), at(y), thickness, h, color);
    canvas.fill_rect(at(x + w - thickness), at(y), thickness, h, color);
}

/// Copies a finished frame of `width` by `height` pixels to the screen,
/// whose rows are `stride` pixels apart, so the person never sees a frame
/// half drawn. Rows or pixels that do not fit are left out.
pub fn present(frame: &[u32], screen: &mut [u32], width: usize, height: usize, stride: usize) {
    if width == 0 || stride < width {
        return;
    }
    for (row, source) in frame.chunks_exact(width).take(height).enumerate() {
        let start = row * stride;
        if let Some(target) = screen.get_mut(start..start + width) {
            target.copy_from_slice(source);
        }
    }
}

/// The first card to show when only `shown` fit, so the chosen one is
/// always among them.
#[must_use]
pub fn first_shown(selected: usize, shown: usize) -> usize {
    (selected + 1).saturating_sub(shown.max(1))
}

/// Draws the whole screen for `cards` in the state `menu` is in.
///
/// # Errors
///
/// The font's errors, which only a broken font file causes.
pub fn draw(
    canvas: &mut Canvas<'_>,
    cards: &[Card<'_>],
    menu: &Menu,
    scratch: &mut Scratch,
) -> Result<(), Error> {
    let regular = Font::parse(INTER)?;
    let semibold = Font::parse(INTER_SEMIBOLD)?;
    let heading = Style {
        font: semibold,
        px: 30.0,
        color: TEXT,
    };
    let name = Style {
        font: regular,
        px: 20.0,
        color: TEXT,
    };
    let letter = Style {
        font: semibold,
        px: 44.0,
        color: INK,
    };
    let hint = Style {
        font: regular,
        px: 18.0,
        color: MUTED,
    };

    canvas.fill(NIGHT);
    let (w, h) = (canvas.width(), canvas.height());
    let (left, top, shown) = cards_layout(w, h, cards.len());
    centred(
        canvas,
        &heading,
        w / 2,
        top.saturating_sub(56),
        "Choose a system",
        scratch,
    )?;

    let first = first_shown(menu.selected(), shown);
    for (slot, (i, card)) in cards.iter().enumerate().skip(first).take(shown).enumerate() {
        let x = left + slot * (CARD_WIDTH + GAP);
        let chosen = i == menu.selected();
        let shadow = if chosen { TANGERINE } else { SHADOW_COLOR };
        canvas.fill_rect(
            at(x + SHADOW),
            at(top + SHADOW),
            CARD_WIDTH,
            CARD_HEIGHT,
            shadow,
        );
        canvas.fill_rect(at(x), at(top), CARD_WIDTH, CARD_HEIGHT, SURFACE);
        let (border, thickness) = if chosen { (TANGERINE, 3) } else { (BORDER, 1) };
        frame(canvas, x, top, CARD_WIDTH, CARD_HEIGHT, thickness, border);

        let tile_x = x + (CARD_WIDTH - TILE) / 2;
        let tile_y = top + 36;
        canvas.fill_rect(at(tile_x), at(tile_y), TILE, TILE, card.kind.color());
        let monogram = card.name.chars().next().unwrap_or('?');
        let mut buf = [0u8; 4];
        centred(
            canvas,
            &letter,
            tile_x + TILE / 2,
            tile_y + TILE / 2 + 16,
            monogram.encode_utf8(&mut buf),
            scratch,
        )?;
        centred(
            canvas,
            &name,
            x + CARD_WIDTH / 2,
            tile_y + TILE + 52,
            card.name,
            scratch,
        )?;
    }

    let below = top + CARD_HEIGHT + SHADOW + 56;
    let mut line = canvas::Text::<96>::new();
    match (menu.seconds_left(), cards.get(menu.selected())) {
        (Some(s), Some(card)) => {
            let _ = core::fmt::write(&mut line, format_args!("{} starts in {s} s", card.name));
        }
        _ => {
            let _ = core::fmt::write(
                &mut line,
                format_args!("Left and Right choose, Enter starts"),
            );
        }
    }
    centred(canvas, &hint, w / 2, below, line.as_str(), scratch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Format;

    const SYSTEMS: [Card<'static>; 3] = [
        Card {
            kind: Kind::Meridian,
            name: "Meridian",
        },
        Card {
            kind: Kind::Windows,
            name: "Windows",
        },
        Card {
            kind: Kind::Linux,
            name: "Arch Linux",
        },
    ];

    struct Picture {
        pixels: Vec<u32>,
        width: usize,
        height: usize,
    }

    impl Picture {
        fn new(width: usize, height: usize) -> Picture {
            Picture {
                pixels: vec![0; width * height],
                width,
                height,
            }
        }

        fn canvas(&mut self) -> Canvas<'_> {
            Canvas::new(
                &mut self.pixels,
                self.width,
                self.height,
                self.width,
                Format::Bgr,
            )
            .unwrap()
        }

        fn at(&mut self, x: usize, y: usize) -> Color {
            self.canvas().pixel(x, y).unwrap()
        }
    }

    fn drawn(cards: &[Card<'_>], menu: &Menu, width: usize, height: usize) -> Picture {
        let mut p = Picture::new(width, height);
        draw(&mut p.canvas(), cards, menu, &mut Scratch::new()).unwrap();
        p
    }

    #[test]
    fn three_cards_sit_in_the_middle() {
        let (left, top, shown) = cards_layout(1280, 800, 3);
        assert_eq!(shown, 3);
        assert_eq!(left, (1280 - 3 * 200 - 2 * 32) / 2);
        assert_eq!(top, (800 - 230) / 2);
        assert_eq!(cards_layout(1280, 800, 1).0, (1280 - 200) / 2);
    }

    #[test]
    fn only_as_many_cards_as_fit_are_shown() {
        assert_eq!(
            cards_layout(1280, 800, 9).2,
            5,
            "five cards of 200 with gaps fit in 1280"
        );
        assert_eq!(cards_layout(640, 480, 9).2, 2);
        assert_eq!(
            cards_layout(100, 100, 3).2,
            1,
            "at least one card, even cut off"
        );
        assert_eq!(cards_layout(1280, 800, 0).2, 0);
    }

    #[test]
    fn each_card_shows_its_tile_and_the_chosen_one_glows() {
        let menu = Menu::new(3, 1, 0);
        let mut p = drawn(&SYSTEMS, &menu, 1280, 800);
        let (left, top, _) = cards_layout(1280, 800, 3);
        for (i, card) in SYSTEMS.iter().enumerate() {
            let x = left + i * (CARD_WIDTH + GAP);
            let tile = (x + (CARD_WIDTH - TILE) / 2, top + 36);
            assert_eq!(
                p.at(tile.0 + 2, tile.1 + 2),
                card.kind.color(),
                "card {i}'s tile"
            );
            assert_eq!(
                p.at(x + 10, top + CARD_HEIGHT - 10),
                SURFACE,
                "card {i}'s face"
            );
            let chosen = i == 1;
            assert_eq!(
                p.at(x, top + 50),
                if chosen { TANGERINE } else { BORDER },
                "card {i}'s border"
            );
            assert_eq!(
                p.at(x + 2, top + 50),
                if chosen { TANGERINE } else { SURFACE },
                "card {i}: thick or thin"
            );
            let shadow = p.at(x + CARD_WIDTH + 2, top + CARD_HEIGHT + 2);
            assert_eq!(
                shadow,
                if chosen { TANGERINE } else { SHADOW_COLOR },
                "card {i}'s shadow"
            );
        }
        assert_eq!(p.at(0, 0), NIGHT);
        assert_eq!(p.at(1279, 799), NIGHT);
    }

    /// Pixels in a band that are neither the background nor `but`.
    fn ink(p: &mut Picture, xs: core::ops::Range<usize>, ys: core::ops::Range<usize>) -> usize {
        let mut n = 0;
        for y in ys {
            for x in xs.clone() {
                n += usize::from(p.at(x, y) != NIGHT);
            }
        }
        n
    }

    #[test]
    fn the_heading_and_the_countdown_are_written() {
        let counting = Menu::new(3, 0, 5);
        let mut p = drawn(&SYSTEMS, &counting, 1280, 800);
        let (_, top, _) = cards_layout(1280, 800, 3);
        let below = top + CARD_HEIGHT + SHADOW + 56;
        assert!(
            ink(&mut p, 400..880, top - 90..top - 50) > 300,
            "the heading"
        );
        let with_countdown = ink(&mut p, 300..980, below - 20..below + 6);
        assert!(with_countdown > 200, "{with_countdown}");

        let waiting = Menu::new(3, 0, 0);
        let mut q = drawn(&SYSTEMS, &waiting, 1280, 800);
        let without = ink(&mut q, 300..980, below - 20..below + 6);
        assert!(
            without > 200 && without != with_countdown,
            "a different line when nothing counts down"
        );
    }

    #[test]
    fn the_countdown_names_the_chosen_system() {
        let mut first = drawn(&SYSTEMS, &Menu::new(3, 0, 5), 1280, 800);
        let mut last = drawn(&SYSTEMS, &Menu::new(3, 2, 5), 1280, 800);
        let (_, top, _) = cards_layout(1280, 800, 3);
        let below = top + CARD_HEIGHT + SHADOW + 56;
        let a = ink(&mut first, 300..980, below - 20..below + 6);
        let b = ink(&mut last, 300..980, below - 20..below + 6);
        assert_ne!(a, b, "\"Meridian starts\" and \"Arch Linux starts\" differ");
    }

    #[test]
    fn the_monogram_is_dark_on_the_tile() {
        let mut p = drawn(&SYSTEMS, &Menu::new(3, 0, 0), 1280, 800);
        let (left, top, _) = cards_layout(1280, 800, 3);
        let tile = (left + (CARD_WIDTH - TILE) / 2, top + 36);
        let mut dark = 0;
        for y in tile.1..tile.1 + TILE {
            for x in tile.0..tile.0 + TILE {
                dark += usize::from(p.at(x, y) == INK);
            }
        }
        assert!(dark > 150, "the M should be drawn in ink: {dark}");
    }

    #[test]
    fn small_screens_and_odd_names_still_draw() {
        let odd = [
            Card {
                kind: Kind::Other,
                name: "",
            },
            Card {
                kind: Kind::Other,
                name: "\\u{1F600} smile",
            },
        ];
        // Drawing must not fail or panic, whatever the screen and names.
        for (w, h) in [(320, 200), (200, 120), (1, 1)] {
            let mut p = drawn(&odd, &Menu::new(2, 1, 3), w, h);
            assert_eq!(p.canvas().width(), w);
        }
        let mut p = drawn(&[], &Menu::new(0, 0, 0), 640, 480);
        assert_eq!(p.at(320, 240), NIGHT, "no cards, nothing in the middle");
    }

    #[test]
    fn frames_go_to_the_screen_row_by_row() {
        let frame: Vec<u32> = (0..12).collect();
        let mut screen = vec![99; 4 * 5];
        present(&frame, &mut screen, 4, 3, 5);
        assert_eq!(&screen[..5], [0, 1, 2, 3, 99], "the padding stays");
        assert_eq!(&screen[5..10], [4, 5, 6, 7, 99]);
        assert_eq!(&screen[10..15], [8, 9, 10, 11, 99]);
        assert_eq!(&screen[15..], [99; 5], "rows past the frame's height stay");
    }

    #[test]
    fn a_frame_larger_than_the_screen_is_cut() {
        let frame = vec![7u32; 4 * 4];
        let mut screen = vec![0; 4 * 2 + 2];
        present(&frame, &mut screen, 4, 4, 4);
        assert_eq!(
            screen,
            [7, 7, 7, 7, 7, 7, 7, 7, 0, 0],
            "a row that does not fit whole is left out"
        );
        let mut untouched = vec![1; 8];
        present(&frame, &mut untouched, 4, 2, 3);
        assert_eq!(untouched, [1; 8], "a stride below the width draws nothing");
        present(&frame, &mut untouched, 0, 2, 3);
        assert_eq!(untouched, [1; 8]);
    }

    #[test]
    fn the_row_follows_the_choice() {
        assert_eq!(first_shown(0, 5), 0);
        assert_eq!(first_shown(4, 5), 0);
        assert_eq!(first_shown(5, 5), 1);
        assert_eq!(first_shown(8, 5), 4);
        assert_eq!(
            first_shown(3, 0),
            3,
            "even when nothing fits, the chosen one is first"
        );
    }

    #[test]
    fn a_chosen_card_past_the_screen_is_scrolled_into_view() {
        let many: Vec<Card<'_>> = (0..8)
            .map(|i| Card {
                kind: if i == 6 { Kind::Linux } else { Kind::Other },
                name: "System",
            })
            .collect();
        let mut menu = Menu::new(8, 0, 0);
        for _ in 0..6 {
            menu.key(crate::menu::Key::Right);
        }
        assert_eq!(menu.selected(), 6);
        let mut p = drawn(&many, &menu, 1280, 800);
        let (left, top, shown) = cards_layout(1280, 800, 8);
        assert_eq!(shown, 5);
        // Cards 2 to 6 are shown, so the chosen one sits in the last slot.
        let x = left + 4 * (CARD_WIDTH + GAP);
        assert_eq!(
            p.at(x + (CARD_WIDTH - TILE) / 2 + 2, top + 38),
            GRASS,
            "the chosen card's tile"
        );
        assert_eq!(p.at(x, top + 50), TANGERINE, "and its frame");
        let first = p.at(left + (CARD_WIDTH - TILE) / 2 + 2, top + 38);
        assert_eq!(first, TEAL, "card 2 in the first slot");
    }

    /// The left and right ends of what is drawn in rows `ys`.
    fn ink_span(p: &mut Picture, ys: core::ops::Range<usize>) -> (usize, usize) {
        let (mut lo, mut hi) = (usize::MAX, 0);
        for y in ys {
            for x in 0..p.width {
                if p.at(x, y) != NIGHT {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
        }
        (lo, hi)
    }

    #[test]
    fn heading_and_hint_are_centred() {
        let mut p = drawn(&SYSTEMS, &Menu::new(3, 0, 0), 1280, 800);
        let (_, top, _) = cards_layout(1280, 800, 3);
        let (lo, hi) = ink_span(&mut p, top - 90..top - 50);
        assert!((lo + hi).abs_diff(1280) <= 4, "heading from {lo} to {hi}");
        let below = top + CARD_HEIGHT + SHADOW + 56;
        let (lo, hi) = ink_span(&mut p, below - 20..below + 6);
        assert!((lo + hi).abs_diff(1280) <= 4, "hint from {lo} to {hi}");
    }

    #[test]
    fn a_nameless_system_gets_a_question_mark() {
        let nameless = [Card {
            kind: Kind::Other,
            name: "",
        }];
        let mut p = drawn(&nameless, &Menu::new(1, 0, 0), 1280, 800);
        let (left, top, _) = cards_layout(1280, 800, 1);
        let tile = (left + (CARD_WIDTH - TILE) / 2, top + 36);
        let mut ink = 0;
        for y in tile.1..tile.1 + TILE {
            for x in tile.0..tile.0 + TILE {
                ink += usize::from(p.at(x, y) == INK);
            }
        }
        assert!(ink > 60, "the question mark should be drawn: {ink}");
    }

    #[test]
    fn every_kind_has_its_own_colour() {
        let colors = [Kind::Meridian, Kind::Windows, Kind::Linux, Kind::Other].map(Kind::color);
        assert_eq!(colors, [TANGERINE, SKY, GRASS, TEAL]);
    }
}
