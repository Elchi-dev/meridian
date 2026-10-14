//! The screen, and room off it to draw in, so the person only ever sees
//! finished frames.

use canvas::Canvas;

pub struct Display<'a> {
    screen: Canvas<'a>,
    back: Option<Canvas<'a>>,
}

impl<'a> Display<'a> {
    /// The screen, drawn off it in `back` when that holds a frame of the
    /// screen's size, and on it otherwise.
    #[must_use]
    pub fn new(screen: Canvas<'a>, back: Option<Canvas<'a>>) -> Display<'a> {
        let back = back.filter(|b| b.width() == screen.width() && b.height() == screen.height());
        Display { screen, back }
    }

    /// Lets `draw` change the rectangle at `x`, `y`, off the screen when
    /// there is room, and then shows the rectangle as `draw` left it.
    pub fn update<T>(
        &mut self,
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        draw: impl FnOnce(&mut Canvas<'a>) -> T,
    ) -> T {
        let Some(back) = self.back.as_mut() else {
            return draw(&mut self.screen);
        };
        let result = draw(back);
        back.copy_to(&mut self.screen, x, y, width, height);
        result
    }

    /// Lets `draw` make a whole new frame, which then replaces the screen.
    pub fn frame<T>(&mut self, draw: impl FnOnce(&mut Canvas<'a>) -> T) -> T {
        self.update(0, 0, usize::MAX, usize::MAX, draw)
    }

    /// What the person sees.
    #[must_use]
    pub fn screen(&self) -> &Canvas<'a> {
        &self.screen
    }

    /// The frame drawn off the screen, when there is room for one.
    #[must_use]
    pub fn back(&self) -> Option<&Canvas<'a>> {
        self.back.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Format;
    use canvas::palette::{NIGHT, SKY, SUN};

    fn canvas(pixels: &mut [u32], w: usize, h: usize) -> Canvas<'_> {
        Canvas::new(pixels, w, h, w, Format::Bgr).unwrap()
    }

    fn colours(c: &Canvas<'_>) -> Vec<Vec<canvas::Color>> {
        (0..c.height())
            .map(|y| (0..c.width()).map(|x| c.pixel(x, y).unwrap()).collect())
            .collect()
    }

    #[test]
    fn a_frame_is_drawn_off_the_screen_and_shown_whole() {
        let (mut front, mut room) = (vec![0; 8 * 4], vec![0; 8 * 4]);
        let mut screen = canvas(&mut front, 8, 4);
        screen.fill(SKY);
        let mut d = Display::new(screen, Some(canvas(&mut room, 8, 4)));
        let got = d.frame(|c| {
            c.fill(NIGHT);
            c.fill_rect(2, 1, 3, 2, SUN);
            7
        });
        assert_eq!(got, 7, "what draw returns comes back");
        assert_eq!(colours(d.screen()), colours(d.back().unwrap()));
        assert_eq!(d.screen().pixel(2, 1), Some(SUN));
        assert_eq!(d.screen().pixel(0, 0), Some(NIGHT));
    }

    #[test]
    fn only_the_rectangle_reaches_the_screen() {
        let (mut front, mut room) = (vec![0; 8 * 4], vec![0; 8 * 4]);
        let mut screen = canvas(&mut front, 8, 4);
        screen.fill(SKY);
        let mut d = Display::new(screen, Some(canvas(&mut room, 8, 4)));
        d.update(1, 1, 2, 2, |c| c.fill(SUN));
        for y in 0..4 {
            for x in 0..8 {
                let inside = (1..3).contains(&x) && (1..3).contains(&y);
                let want = if inside { SUN } else { SKY };
                assert_eq!(d.screen().pixel(x, y), Some(want), "({x}, {y})");
            }
        }
        assert_eq!(d.back().unwrap().pixel(7, 3), Some(SUN), "drawn off screen");
    }

    #[test]
    fn without_room_drawing_goes_to_the_screen() {
        let mut front = vec![0; 8 * 4];
        let mut d = Display::new(canvas(&mut front, 8, 4), None);
        d.update(0, 0, 1, 1, |c| c.fill(SUN));
        assert!(d.back().is_none());
        assert_eq!(
            d.screen().pixel(7, 3),
            Some(SUN),
            "all of it, not just the rectangle"
        );
    }

    #[test]
    fn room_of_another_size_is_not_used() {
        for (w, h) in [(7, 4), (8, 3), (9, 4)] {
            let (mut front, mut room) = (vec![0; 8 * 4], vec![0; w * h]);
            let mut d = Display::new(canvas(&mut front, 8, 4), Some(canvas(&mut room, w, h)));
            assert!(d.back().is_none(), "{w} by {h}");
            d.frame(|c| c.fill(SUN));
            assert_eq!(d.screen().pixel(7, 3), Some(SUN));
        }
    }
}
