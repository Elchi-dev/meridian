# 0012. Use Inter and JetBrains Mono as the system fonts

- Status: accepted
- Date: 2026-10-14
- Allows: third-party/fonts/Inter-Regular.ttf, third-party/fonts/Inter-Medium.ttf, third-party/fonts/Inter-SemiBold.ttf, third-party/fonts/Inter-Bold.ttf, third-party/fonts/Inter-OFL.txt, third-party/fonts/JetBrainsMono-Regular.ttf, third-party/fonts/JetBrainsMono-Bold.ttf, third-party/fonts/JetBrainsMono-OFL.txt

## Context

The boot screen, the status screen and the panic screen draw text from M1 on, and Horizon draws all of its text later. Meridian writes its own font parser and rasterizer (`lib/font`), but a typeface is not code: drawing hundreds of glyphs that read well at every size, with spacing and kerning tuned by hand, is years of design work. That is exactly the kind of outside file 0003 allows with a record.

Samuel chose Inter for the interface and JetBrains Mono for the terminal (QUESTIONS.md 11.4), with one record for all fonts.

## Decision

Meridian ships these files, unchanged, under `third-party/fonts/`:

| File | From | SHA-256 |
| --- | --- | --- |
| `Inter-Regular.ttf` | Inter 4.1, `extras/ttf/` | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `Inter-Medium.ttf` | Inter 4.1, `extras/ttf/` | `97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872` |
| `Inter-SemiBold.ttf` | Inter 4.1, `extras/ttf/` | `78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3` |
| `Inter-Bold.ttf` | Inter 4.1, `extras/ttf/` | `288316099b1e0a47a4716d159098005eef7c0066921f34e3200393dbdb01947f` |
| `Inter-OFL.txt` | Inter 4.1, `LICENSE.txt` | `262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a` |
| `JetBrainsMono-Regular.ttf` | JetBrains Mono 2.304, `fonts/ttf/` | `a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f` |
| `JetBrainsMono-Bold.ttf` | JetBrains Mono 2.304, `fonts/ttf/` | `5590990c82e097397517f275f430af4546e1c45cff408bde4255dad142479dcb` |
| `JetBrainsMono-OFL.txt` | JetBrains Mono 2.304, `OFL.txt` | `30f0c136e3c88e422d0791acd97238870f9054a9729bc34cf2ff0d4ed8cac4ad` |

The releases, as published by their projects:

- Inter 4.1 (16 November 2024): https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip, SHA-256 `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`
- JetBrains Mono 2.304 (14 January 2023): https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip, SHA-256 `6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf`

Both are licensed under the SIL Open Font License 1.1, which allows bundling the fonts with software under any licence, as long as the licence text travels with them and the fonts are not sold on their own. The licence texts sit next to the fonts.

Inter's main download is a variable font; the static instances in its `extras/ttf/` folder are the project's own builds of the same design and use TrueType outlines (`glyf`), which `lib/font` reads. All six files carry `glyf` outlines. Neither family has a `kern` table: their kerning lives in OpenType `GPOS`, which `lib/font` does not read yet, so text is set without kerning until it does.

## Alternatives considered

- **Designing Meridian's own typeface.** The only way to have no outside file here, and far too large a project for an operating system that has no kernel yet.
- **Bitmap fonts drawn by hand.** No outside file and easy to render, but they look dated at modern sizes and cannot scale with the display, which Horizon must.
- **The variable font files.** One file per family instead of several, but reading `gvar` variations is a large addition to the parser, and four fixed weights are all the boot screens and Horizon's first version need.
- **Other families** (Noto, Source Sans, IBM Plex). All fine; Samuel picked Inter and JetBrains Mono.

## Consequences

The fonts add 2.2 MiB to the repository. Updating them is a deliberate change of the files, their hashes here, and the `Allows:` line. Text has no kerning until `GPOS` is supported.
