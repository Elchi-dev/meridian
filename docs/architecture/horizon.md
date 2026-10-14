# Horizon, the desktop

Planned for M7. Before any compositor code, the look and the flows were
built as a clickable prototype; its values below are what the real code
starts from.

## Layout

The layout is closer to Windows than to macOS, but the look is Meridian's
own:

- a taskbar at the bottom with the launcher, search, pinned and running apps, workspaces, tray and clock
- window buttons on the right, menus inside windows
- a launcher that searches apps, files and settings
- a window overview with workspaces
- a notification centre and a control centre (volume, brightness, network)
- widgets on the desktop

A global menu bar exists only as an experiment, for Windows programs.

## Look

- **Surfaces and borders.** Opaque surfaces, crisp 1 px borders, depth from a solid offset shadow.
- **Never.** No gradients, no blur, no glass, no neon glow, no purple.
- **Colour.** A saturated palette where each app owns one colour for its title bar, taskbar tile and widget header:

  | Name | Light | Dark |
  | --- | --- | --- |
  | tomato | `#FF4B39` | `#FF5B4A` |
  | tangerine (default accent) | `#FF8A1C` | `#FF9633` |
  | sun | `#FFC62B` | `#FFCE45` |
  | lime | `#A3D52A` | `#B2E040` |
  | grass | `#22B45E` | `#33C670` |
  | teal | `#12A4A1` | `#1DB9B6` |
  | sky | `#2F7CF6` | `#4A8DFF` |
  | pink | `#FF4E98` | `#FF66A8` |

- **Text on colour** is always dark, in both themes.
- **Fixed looks.** The boot screens keep a fixed dark look, and the panic screen is sun yellow.
- **Everything can be changed** in Settings: light or dark, accent, palette, corner radius, density, wallpaper.

## Motion

One spring model drives every movement: windows opening, closing, minimising
and maximising, dragging and snapping, the overview, workspace changes. With
reduced motion turned on, everything snaps to its end state.

## Drawing

- **First on the CPU.** A compositor draws into the framebuffer the firmware hands over, with SIMD and only the changed areas redrawn. That is enough for 1080p and 1440p at 60 Hz, not for 4K at 120 Hz.
- **Then virtio-gpu.** It comes in QEMU first, with several screens.
- **Real GPU drivers come later.** AMD GPUs need AMD's signed firmware files. Until then, real hardware shows one monitor, the one UEFI hands over.

## Input that never freezes

The compositor and the cursor thread get reserved CPU time and memory from
the scheduler, so a program at 100 % CPU on every core cannot make the
mouse stutter.

## Text and icons

- **Text.** Meridian's own TrueType and OpenType rasterizer, grayscale anti-aliasing, no hinting. Inter for the interface and JetBrains Mono for the terminal, both under the SIL Open Font License, both in one decision record. Arabic and Indic shaping and colour emoji come later.
- **Icons.** Vector icons, drawn by a renderer for a small SVG subset, so they stay sharp at every scale.

## Keyboard and pointer

- **Shortcuts.** Ctrl shortcuts as on Windows by default; Super as Cmd is a setting.
- **Layouts.** Every layout comes from Unicode's CLDR data, under its own decision record.
- **Scrolling and touchpads.** Natural scrolling is on by default. Touchpad gestures need the I2C HID driver, which comes after ACPI.

## First apps

- Terminal
- Files, with the time slider and installer undo
- Settings, with an Experiments page
- System monitor, with every process, handle, driver and mapping and nothing hidden
- Launcher
- Image viewer
- Clock and calendar
- Widgets

## The UI toolkit

Declarative, in Rust: an app describes its interface for a state, and the
toolkit updates the screen. Native apps use it, and so do the standard
controls of Windows programs.
