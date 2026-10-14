# Fonts

`lib/font` reads TrueType fonts, lays out a line of text and rasterizes
glyphs with grayscale anti-aliasing. The loader draws its boot screen with
it in M1; the kernel's status and panic screens and Horizon use it later.

It is a `no_std` crate with no `unsafe` code and no dependencies, and it
never allocates: a `Font` borrows the file's bytes, outlines go to a sink
the caller provides, and glyphs are rasterized into the caller's buffers.
So it works before there is a heap, and the same code runs in the loader,
the kernel and user space.

The fonts themselves, Inter and JetBrains Mono, are the only outside files
and come under [ADR 0012](../adr/0012-fonts.md).

## What it reads

| Table | What for |
| --- | --- |
| `head` | units per em, the format of `loca`, and the magic number as a check |
| `maxp` | the number of glyphs |
| `hhea` | ascent, descent, line gap, and how many glyphs have a full `hmtx` record |
| `hmtx` | advance widths and left side bearings; glyphs past the last full record share its advance |
| `cmap` | characters to glyphs, from format 12 (every plane) when there is one, else format 4 (the first plane) |
| `loca` | where each glyph's outline is, in either the short or the long format |
| `glyf` | outlines: simple glyphs, and composite glyphs that move, scale, stretch or turn other glyphs, nested up to 8 deep |
| `kern` | kerning pairs, from a first subtable in format 0 with plain horizontal kerning |

Every read is checked against the end of its table and the table against
the end of the file. A font that is cut short, has bits flipped or was
made to attack the reader gives an `Error`, never a panic. Some limits keep
hostile fonts from making work without end: composites nest at most 8
deep and draw at most 64 components in all, and glyphs rasterize at pixel
sizes up to 1024.

## What it does not do yet

- **Hinting.** The TrueType instructions in `fpgm`, `prep`, `cvt` and in
  each glyph are skipped. Glyphs are drawn from their outlines as
  designed, scaled to the pixel size. At small sizes stems fall between
  pixels and look softer than in a hinting renderer. Inter and JetBrains
  Mono are designed to read well without hints.
- **CFF outlines.** OpenType fonts with PostScript outlines (`CFF `,
  `CFF2`, files that start with `OTTO`) are refused as `NotTrueType`.
- **GPOS kerning.** Modern fonts keep their kerning in OpenType `GPOS`.
  Neither shipped family has a `kern` table, so text in them is set without
  kerning until `GPOS` pair adjustment is read.
- **Shaping.** One character gives one glyph, set left to right: no
  ligatures, no contextual forms, no combining marks placed by `GPOS`, no
  right-to-left text and none of the scripts that need shaping, such as
  Arabic or the Indic scripts.
- **Colour glyphs.** No `COLR`, `CPAL`, `sbix`, `CBDT` or `SVG `, so no
  colour emoji.
- **The rest.** No variable fonts (`fvar`, `gvar`), no vertical metrics
  (`vhea`, `vmtx`), no font collections (`.ttc`), no `cmap` formats other
  than 4 and 12 (so no variation selectors from format 14), and no
  composite glyphs that place components by matching points, which give
  `PointMatching`.

## How a glyph is drawn

1. **Scale.** The pixel size is the height of one em, so font units become
   pixels by `px / units_per_em`.
2. **Placement.** The glyph's box from its `glyf` header, scaled and moved
   by the pen's fraction of a pixel, rounded outwards to whole pixels,
   gives the bitmap's size and where its top left corner goes relative to
   the pen on the baseline.
3. **Outline.** The points of each contour are decoded and turned into
   lines and quadratic curves. Two off-curve points in a row imply an
   on-curve point halfway between them. Composite glyphs draw their
   components with each one's offset and 2 by 2 matrix applied.
4. **Flattening.** Each curve becomes `n` straight lines. A curve split
   into `n` pieces strays from its lines by at most a quarter of
   `p0 - 2c + p2` over `n` squared, so `n` grows until that is within
   0.05 pixels, up to 64 lines.
5. **Accumulating.** Every line adds, to each row of pixels it crosses,
   the area it encloses to its right in that row, signed by its direction.
   The rasterizer stores this as differences between neighbouring pixels,
   so a line costs a few additions per pixel it touches. A line that
   crosses the left or right side of the bitmap is cut there, and the part
   outside is pressed onto the side, which keeps the coverage inside
   exact.
6. **Coverage.** A running sum along each row turns the differences into
   the covered area of each pixel, from 0 to 1. Lines running up and down
   cancel, so holes come out empty. The absolute value, scaled to 0 to 255
   and rounded, is the pixel's coverage; parts that overlap stop at 255.

The coverage is the covered area, linear in light: a pixel half covered
is 128.
How it is blended into the framebuffer is up to the caller.

## How it is tested

- **Exact shapes.** Squares on and between pixels, concave shapes, holes,
  shapes past the bitmap's edges and hundreds of random triangles are
  compared pixel by pixel with their exact area, found by clipping each
  polygon to each pixel. A circle of 16 curves matches pi r squared
  within what the curves and flattening allow.
- **The shipped fonts.** Units per em, advance widths, side bearings, line
  metrics and every `cmap` entry are checked against a reader in the tests
  that walks the tables front to back. The format 4 and format 12 tables
  must agree on every character of the first plane. For glyphs such as
  `o`, `a`, `ä`, `ß` and `€`, the coverage adds up to the outline's exact
  area; straight glyphs such as `H` match the exact area pixel by pixel;
  `o`, `I` and `H` render symmetric; and every glyph's points stay within
  its box, which catches misplaced components.
- **Built fonts.** Fonts assembled byte by byte in the tests cover what the
  shipped ones lack: every kind of composite, both `loca` formats, short
  `hmtx`, format 4 glyph lists, `kern`, and each way a table can be broken.
- **Broken fonts.** A small font is cut at every length and has every bit
  flipped in turn; the real Inter is cut short and has bits flipped where
  the reader looks. Nothing may panic or take longer than it should.
- **Mutation testing.** Changing the code in ways that should matter must
  make a test fail.

## Speed

About 620 000 glyphs per second at 16 px: Inter Regular, the printable
ASCII characters at four pen positions between pixels, on one core of a
Ryzen 5 5600G in a release build (the `glyphs_per_second_at_16_px` test).
A full screen of text is a few thousand glyphs, so drawing it takes a few
milliseconds even without a cache.

## Drawing text in the loader

- Parse each font once, from `include_bytes!`. `Font` is `Copy` and holds
  only slices, so passing it around costs nothing.
- `Font::line_metrics(px)` gives the ascent above the baseline, the descent
  below it and the gap to the next line. Put the baseline at the line's
  top plus the ascent, rounded to a whole pixel, and the next line one
  `line_height()` further down.
- `Font::layout(text, px, |p| ...)` hands over each glyph with its pen
  position from the start of the line, and returns the line's width for
  centring or right alignment. For each glyph, split the pen position into
  whole pixels and a fraction, and rasterize with the fraction as `shift`;
  text then keeps its designed spacing instead of drifting by rounding.
- `Font::placement` says how large a glyph's bitmap is. Scratch space is
  `Placement::scratch_len()` floats and the bitmap width times height
  bytes. At 16 px the largest glyph of the shipped fonts needs about 620
  floats (2.4 KiB) of scratch space and 600 bytes of bitmap; at 32 px,
  about 2300 floats (9 KiB) and 2250 bytes. Fixed buffers on the stack,
  sized for the largest size the screen uses, are enough;
  `BufferTooSmall` says when they are not.
- The bitmap's top left pixel goes `left` pixels right of the pen's whole
  pixel and `top` pixels below the baseline (`top` is negative for glyphs
  above it). Blend each pixel as `background + (colour - background) *
  coverage / 255`. For the most even weight blend in linear light;
  blending the sRGB values directly is cheaper, but makes dark text on a
  light background a little heavier and light text on dark a little
  thinner.
- The boot screen draws the same few strings many times. Rasterizing them
  once into a cache, by glyph, size and the shift rounded to a quarter of
  a pixel, saves the work on every frame.
- A character the font lacks comes out as the missing-glyph box, glyph 0.
  For the menu that is the right signal; for names read from disks, such as
  other systems' labels, it shows where the font falls short.
