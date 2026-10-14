//! Horizon's colours (docs/architecture/horizon.md). The boot screens keep
//! a fixed dark look; the panic screen is sun yellow with dark text.

#![expect(
    clippy::unreadable_literal,
    reason = "colours read as RRGGBB, the way horizon.md lists them"
)]

use crate::Color;

pub const TOMATO: Color = Color::hex(0xFF5B4A);
pub const TANGERINE: Color = Color::hex(0xFF9633);
pub const SUN: Color = Color::hex(0xFFC62B);
pub const LIME: Color = Color::hex(0xB2E040);
pub const GRASS: Color = Color::hex(0x33C670);
pub const TEAL: Color = Color::hex(0x1DB9B6);
pub const SKY: Color = Color::hex(0x4A8DFF);
pub const PINK: Color = Color::hex(0xFF66A8);

/// The dark look's background, surfaces, borders and text.
pub const NIGHT: Color = Color::hex(0x15171A);
pub const SURFACE: Color = Color::hex(0x22252A);
pub const BORDER: Color = Color::hex(0x34383F);
pub const TEXT: Color = Color::hex(0xF2F3F5);
pub const MUTED: Color = Color::hex(0xA7ADB7);
/// Text on any colour of the palette.
pub const INK: Color = Color::hex(0x16181B);
