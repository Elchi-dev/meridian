//! The panic screen: what went wrong, in words, and a QR code of a short
//! crash summary that a phone can read off the screen. It is sun yellow,
//! so it is never taken for anything else.

use canvas::palette::{INK, SUN};
use canvas::{Canvas, Color, Scratch, Style};
use font::{Error, Font};
use qr::{Code, Ecc};

static INTER: &[u8] = include_bytes!("../../third-party/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../third-party/fonts/Inter-SemiBold.ttf");
static MONO: &[u8] = include_bytes!("../../third-party/fonts/JetBrainsMono-Regular.ttf");

/// What the screen tells.
pub struct Details<'a> {
    /// The panic message, such as "index out of bounds".
    pub message: &'a str,
    /// Where it happened: file, line and column.
    pub location: &'a str,
}

const MARGIN: i32 = 56;
/// The light square around the code, which readers need to find it.
const QUIET: usize = 4;
const LIGHT: Color = Color::hex(0x00ff_ffff);

/// The text the QR code holds. Short, so the code stays easy to read.
///
/// # Errors
///
/// Fails when the writer does, such as a full [`crate::Text`].
pub fn summary(details: &Details<'_>, out: &mut impl core::fmt::Write) -> core::fmt::Result {
    write!(
        out,
        "Meridian {} stopped\nevent: panic\nmessage: {}\nat: {}\n",
        crate::VERSION,
        details.message,
        details.location
    )
}

/// Splits `text` into lines no wider than `width` pixels, breaking at
/// spaces, and hands each line to `line`. A word wider than the line goes
/// on a line of its own.
///
/// # Errors
///
/// The font's errors.
pub fn wrap(
    font: &Font<'_>,
    px: f32,
    text: &str,
    width: u32,
    mut line: impl FnMut(&str) -> Result<(), Error>,
) -> Result<(), Error> {
    // Line widths on a screen are far below where f32 loses whole pixels.
    #[allow(clippy::cast_precision_loss)]
    let width = width as f32;
    let fits = |s: &str| -> Result<bool, Error> { Ok(font.layout(s, px, |_| {})? <= width) };
    let mut start = 0;
    let mut end = 0;
    for (at, _) in text
        .match_indices(' ')
        .chain(core::iter::once((text.len(), "")))
    {
        if end > start && !fits(&text[start..at])? {
            line(&text[start..end])?;
            start = end + 1;
        }
        end = at;
    }
    if start < text.len() {
        line(&text[start..])?;
    }
    Ok(())
}

/// Draws the whole screen, with `code` and `scratch` as working space.
///
/// # Errors
///
/// The font's errors, which only a broken font file causes.
pub fn draw(
    canvas: &mut Canvas<'_>,
    details: &Details<'_>,
    code: &mut Code,
    scratch: &mut Scratch,
) -> Result<(), Error> {
    canvas.fill(SUN);
    let title = Style {
        font: Font::parse(INTER_SEMIBOLD)?,
        px: 40.0,
        color: INK,
    };
    let body = Style {
        font: Font::parse(INTER)?,
        px: 20.0,
        color: INK,
    };
    let mono = Style {
        font: Font::parse(MONO)?,
        px: 16.0,
        color: INK,
    };

    let mut summary_text = crate::Text::<480>::new();
    let _ = summary(details, &mut summary_text);
    let has_code = qr::encode(summary_text.as_str().as_bytes(), Ecc::Medium, code).is_ok();
    let side = if has_code { code.size() + 2 * QUIET } else { 0 };
    let module = ((canvas.height() / 2) / side.max(1)).clamp(1, 8);
    let code_px = side * module;
    let text_width = canvas
        .width()
        .saturating_sub(2 * MARGIN as usize + code_px + if has_code { 48 } else { 0 });

    canvas.text(&title, MARGIN, MARGIN + 40, "Meridian stopped", scratch)?;
    let mut baseline = MARGIN + 96;
    let width = u32::try_from(text_width).unwrap_or(u32::MAX);
    let explanation = "Something went wrong inside the kernel, and Meridian stopped before it could do any harm. Restart the computer. If this keeps happening, scan the code and send what it says along with your report.";
    wrap(&body.font, body.px, explanation, width, |line| {
        canvas.text(&body, MARGIN, baseline, line, scratch)?;
        baseline += 30;
        Ok(())
    })?;
    baseline += 24;
    for (label, text) in [
        ("What happened", details.message),
        ("Where", details.location),
    ] {
        canvas.text(&body, MARGIN, baseline, label, scratch)?;
        baseline += 26;
        wrap(&mono.font, mono.px, text, width, |line| {
            canvas.text(&mono, MARGIN, baseline, line, scratch)?;
            baseline += 22;
            Ok(())
        })?;
        baseline += 16;
    }

    if has_code {
        let left = canvas.width().saturating_sub(MARGIN as usize + code_px);
        let top = MARGIN as usize;
        let at = |n: usize| i32::try_from(n).unwrap_or(i32::MAX / 2);
        canvas.fill_rect(at(left), at(top), code_px, code_px, LIGHT);
        for y in 0..code.size() {
            for x in 0..code.size() {
                if code.is_dark(x, y) {
                    let px = left + (QUIET + x) * module;
                    let py = top + (QUIET + y) * module;
                    canvas.fill_rect(at(px), at(py), module, module, INK);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Format;
    use std::process::Command;

    fn details() -> Details<'static> {
        Details {
            message: "index out of bounds: the len is 3 but the index is 7",
            location: "kernel/src/screen.rs:42:9",
        }
    }

    #[test]
    fn the_summary_is_short_and_plain() {
        let mut out = String::new();
        summary(&details(), &mut out).unwrap();
        assert_eq!(
            out,
            format!(
                "Meridian {} stopped\nevent: panic\nmessage: index out of bounds: the len is 3 but the index is 7\nat: kernel/src/screen.rs:42:9\n",
                crate::VERSION
            )
        );
    }

    #[test]
    fn the_summary_takes_whatever_it_is_given() {
        let d = Details {
            message: "",
            location: "",
        };
        let mut out = String::new();
        summary(&d, &mut out).unwrap();
        assert!(out.ends_with("message: \nat: \n"), "{out:?}");
        let mut short = crate::Text::<20>::new();
        assert!(
            summary(&details(), &mut short).is_err(),
            "a full buffer reports it"
        );
        assert_eq!(short.as_str(), "Meridian 0.0.0 stopp");
    }

    fn lines(text: &str, width: u32) -> Vec<String> {
        let font = Font::parse(INTER).unwrap();
        let mut out = Vec::new();
        wrap(&font, 20.0, text, width, |l| {
            out.push(l.to_string());
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn wrapping_breaks_at_spaces_within_the_width() {
        let font = Font::parse(INTER).unwrap();
        let text = "Something went wrong inside the kernel, and Meridian stopped before it could do any harm.";
        let got = lines(text, 300);
        assert!(got.len() >= 3, "{got:?}");
        assert_eq!(got.join(" "), text, "nothing lost, nothing added");
        for l in &got {
            assert!(
                font.layout(l, 20.0, |_| {}).unwrap() <= 300.0,
                "{l:?} is too wide"
            );
            assert!(!l.starts_with(' ') && !l.ends_with(' '), "{l:?}");
        }
        let joined_pairs: Vec<String> = got
            .windows(2)
            .map(|w| format!("{} {}", w[0], w[1].split(' ').next().unwrap()))
            .collect();
        for pair in joined_pairs {
            assert!(
                font.layout(&pair, 20.0, |_| {}).unwrap() > 300.0,
                "{pair:?} would have fit on one line"
            );
        }
    }

    #[test]
    fn a_long_word_gets_a_line_of_its_own() {
        let got = lines(
            "at kernel/src/a/very/long/path/that/never/fits.rs:1:1 here",
            120,
        );
        assert_eq!(
            got,
            [
                "at",
                "kernel/src/a/very/long/path/that/never/fits.rs:1:1",
                "here"
            ]
        );
        let first_too_wide = lines(
            "kernel/src/a/very/long/path/that/never/fits.rs:1:1 here",
            120,
        );
        assert_eq!(
            first_too_wide,
            ["kernel/src/a/very/long/path/that/never/fits.rs:1:1", "here"]
        );
        assert_eq!(lines("", 100), Vec::<String>::new());
        assert_eq!(lines("one", 100), ["one"]);
    }

    struct Picture {
        pixels: Vec<u32>,
        width: usize,
        height: usize,
    }

    impl Picture {
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
    }

    fn drawn(width: usize, height: usize, details: &Details<'_>) -> (Picture, Code) {
        let mut picture = Picture {
            pixels: vec![0; width * height],
            width,
            height,
        };
        let mut code = Code::new();
        draw(
            &mut picture.canvas(),
            details,
            &mut code,
            &mut Scratch::new(),
        )
        .unwrap();
        (picture, code)
    }

    #[test]
    fn the_screen_is_sun_yellow_with_dark_text() {
        let (mut picture, _) = drawn(1280, 800, &details());
        let c = picture.canvas();
        assert_eq!(c.pixel(0, 0), Some(SUN));
        assert_eq!(c.pixel(1279, 799), Some(SUN));
        let ink = (56..400)
            .flat_map(|x| (60..100).map(move |y| (x, y)))
            .filter(|&(x, y)| c.pixel(x, y).is_some_and(|p| p.r < 0x40))
            .count();
        assert!(ink > 300, "the title should be dark on yellow: {ink}");
    }

    /// The QR code on the screen, read back module by module from the
    /// pixels at each module's centre.
    #[test]
    fn the_code_on_screen_is_the_code_encoded() {
        let (mut picture, code) = drawn(1280, 800, &details());
        assert_eq!(
            code.ecc(),
            Ecc::Medium,
            "a scratched screen or a blurry photo still reads"
        );
        let c = picture.canvas();
        let side = code.size() + 2 * QUIET;
        let module = (400 / side).clamp(1, 8);
        let left = 1280 - 56 - side * module;
        let top = 56;
        for y in 0..side {
            for x in 0..side {
                let at = c
                    .pixel(
                        left + x * module + module / 2,
                        top + y * module + module / 2,
                    )
                    .unwrap();
                let dark = code.is_dark(x.wrapping_sub(QUIET), y.wrapping_sub(QUIET));
                assert_eq!(at, if dark { INK } else { LIGHT }, "module ({x}, {y})");
            }
        }
    }

    #[test]
    fn zbar_reads_the_summary_off_the_screen() {
        if Command::new("zbarimg").arg("--version").output().is_err() {
            assert!(
                std::env::var("SEXTANT_REQUIRE_TOOLS").as_deref() != Ok("1"),
                "zbarimg is required"
            );
            return;
        }
        let d = details();
        let (mut picture, _) = drawn(1024, 768, &d);
        let c = picture.canvas();
        let mut pgm = format!("P5\n{} {}\n255\n", 1024, 768).into_bytes();
        for y in 0..768 {
            for x in 0..1024 {
                let p = c.pixel(x, y).unwrap();
                let gray = (u32::from(p.r) * 3 + u32::from(p.g) * 6 + u32::from(p.b)) / 10;
                pgm.push(u8::try_from(gray).unwrap());
            }
        }
        let path = std::env::temp_dir().join(format!("meridian-panic-{}.pgm", std::process::id()));
        std::fs::write(&path, pgm).unwrap();
        let out = Command::new("zbarimg")
            .args(["-q", "--raw", "-Sbinary"])
            .arg(&path)
            .output()
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut want = String::new();
        summary(&d, &mut want).unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want);
    }

    #[test]
    fn small_screens_and_long_messages_still_draw() {
        let long = "x".repeat(2000);
        let words = "word ".repeat(300);
        for (w, h) in [(320, 200), (640, 480), (64, 64)] {
            for message in [long.as_str(), words.as_str(), ""] {
                let d = Details {
                    message,
                    location: "here",
                };
                let (mut picture, _) = drawn(w, h, &d);
                assert_eq!(picture.canvas().pixel(0, 0), Some(SUN), "{w} by {h}");
            }
        }
    }

    #[test]
    fn text_keeps_clear_of_the_code() {
        let long = Details {
            message: &"a long message that has to wrap many times ".repeat(8),
            location: "kernel/src/somewhere/deep/inside/the/tree.rs:1234:56",
        };
        let (mut picture, code) = drawn(1280, 800, &long);
        let c = picture.canvas();
        let side = code.size() + 2 * QUIET;
        let module = (400 / side).clamp(1, 8);
        let code_left = 1280 - 56 - side * module;
        // The text column ends 48 pixels before the code; nothing is drawn
        // in that gap.
        for y in 0..800 {
            for x in code_left - 47..code_left {
                assert_eq!(c.pixel(x, y), Some(SUN), "({x}, {y})");
            }
        }
        let below = (56..code_left - 48).flat_map(|x| (380..460).map(move |y| (x, y)));
        assert!(
            below.filter(|&(x, y)| c.pixel(x, y) != Some(SUN)).count() > 100,
            "the message wrapped down the screen"
        );
    }

    #[test]
    fn a_message_too_long_for_the_code_is_cut_short() {
        let long = "y".repeat(3000);
        let d = Details {
            message: &long,
            location: "here",
        };
        let (_, code) = drawn(1280, 800, &d);
        let mut full = String::new();
        summary(&d, &mut full).unwrap();
        assert!(full.len() > 3000);
        // The summary stops at 480 bytes, which needs version 17: still
        // large modules on any screen.
        assert!(code.version() <= 17, "version {}", code.version());
    }

    /// The rows of the text column with ink in them, as bands from their
    /// first row to their last.
    fn bands(
        pixels: &mut [u32],
        width: usize,
        height: usize,
        columns: core::ops::Range<usize>,
    ) -> Vec<(usize, usize)> {
        let canvas = Canvas::new(pixels, width, height, width, Format::Bgr).unwrap();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for y in 0..height {
            let ink = columns.clone().any(|x| canvas.pixel(x, y) != Some(SUN));
            match out.last_mut() {
                Some(band) if ink && band.1 + 1 == y => band.1 = y,
                _ if ink => out.push((y, y)),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn every_line_of_the_text_stands_apart() {
        let (w, h) = (1280, 800);
        let mut px = vec![0u32; w * h];
        let mut canvas = Canvas::new(&mut px, w, h, w, Format::Bgr).unwrap();
        let d = Details {
            message: "a short message",
            location: "kernel/src/x.rs:1:1",
        };
        draw(&mut canvas, &d, &mut Code::new(), &mut Scratch::new()).unwrap();
        // The text column ends well before the code on the right.
        let found = bands(&mut px, w, h, MARGIN as usize..600);
        let explanation = "Something went wrong inside the kernel, and Meridian stopped before it could do any harm. Restart the computer. If this keeps happening, scan the code and send what it says along with your report.";
        let text_width = {
            let mut code = Code::new();
            let mut summary_text = crate::Text::<480>::new();
            let _ = summary(&d, &mut summary_text);
            qr::encode(summary_text.as_str().as_bytes(), Ecc::Medium, &mut code).unwrap();
            let side = code.size() + 2 * QUIET;
            let module = ((h / 2) / side).clamp(1, 8);
            w - 2 * MARGIN as usize - side * module - 48
        };
        let explained = lines(explanation, u32::try_from(text_width).unwrap()).len();
        // The title, the explanation, then a label and a line for each of
        // the message and the place.
        assert_eq!(found.len(), 1 + explained + 4, "{found:?}");
        for pair in found.windows(2) {
            assert!(pair[1].0 > pair[0].1 + 2, "lines too close: {pair:?}");
        }
    }

    #[test]
    fn a_line_exactly_as_wide_as_the_room_fits() {
        let font = Font::parse(INTER).unwrap();
        // Advances are whole font units, so some text is a whole number of
        // pixels wide; that is the case where a line just fits.
        let text = (1..200)
            .map(|n| format!("{} {}", "x".repeat(n), "x".repeat(n)))
            .find(|t| font.layout(t, 20.0, |_| {}).unwrap().fract() == 0.0)
            .expect("some text a whole number of pixels wide");
        let width = font.layout(&text, 20.0, |_| {}).unwrap();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let got = lines(&text, width as u32);
        assert_eq!(got, std::slice::from_ref(&text), "it fits exactly");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let narrower = lines(&text, width as u32 - 1);
        assert_eq!(narrower.len(), 2);
    }
}
