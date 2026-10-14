//! `EFI_GRAPHICS_OUTPUT_PROTOCOL`, the firmware's framebuffer (UEFI 2.10,
//! section 12.9).

use core::ptr::{self, NonNull};

use super::System;
use super::table::NotCalled;
use crate::Status;
use crate::guid::GRAPHICS_OUTPUT_PROTOCOL;

#[repr(C)]
pub struct GraphicsOutput {
    pub query_mode: NotCalled,
    pub set_mode: NotCalled,
    pub blt: NotCalled,
    pub mode: *mut Mode,
}

#[repr(C)]
pub struct Mode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut ModeInfo,
    pub size_of_info: usize,
    pub frame_buffer_base: u64,
    pub frame_buffer_size: usize,
}

#[repr(C)]
pub struct ModeInfo {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub pixel_information: [u32; 4],
    pub pixels_per_scan_line: u32,
}

/// How the four bytes of a pixel are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// Red, green, blue, unused.
    Rgb,
    /// Blue, green, red, unused.
    Bgr,
}

impl PixelFormat {
    /// The format for a UEFI pixel format number. Formats given by bit
    /// masks, and screens only reachable through `Blt`, are not drawn on.
    #[must_use]
    pub fn from_uefi(format: u32) -> Option<PixelFormat> {
        match format {
            0 => Some(PixelFormat::Rgb),
            1 => Some(PixelFormat::Bgr),
            _ => None,
        }
    }
}

/// The framebuffer of the current graphics mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Framebuffer {
    /// Physical address of the top left pixel.
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Pixels from the start of one row to the start of the next.
    pub stride: u32,
    pub format: PixelFormat,
}

impl Framebuffer {
    /// How many pixels there are from the first to the last visible one.
    /// The last row may end without the padding the others have.
    #[must_use]
    pub fn pixel_count(&self) -> u64 {
        if self.height == 0 {
            return 0;
        }
        u64::from(self.height - 1) * u64::from(self.stride) + u64::from(self.width)
    }

    /// Whether every visible pixel lies in the memory the firmware reports.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.stride >= self.width
            && self
                .pixel_count()
                .checked_mul(4)
                .is_some_and(|bytes| bytes <= self.size)
    }
}

impl System {
    /// The framebuffer of the graphics mode the firmware set.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` without graphics output, `UNSUPPORTED` for a screen
    /// without a framebuffer Meridian can draw on, and
    /// `VOLUME_CORRUPTED` for mode information that does not add up.
    pub fn framebuffer(&self) -> Result<Framebuffer, Status> {
        let mut interface = ptr::null_mut();
        // SAFETY: LocateProtocol writes one interface pointer; the
        // registration is unused.
        unsafe {
            (self.boot_services().locate_protocol)(
                &GRAPHICS_OUTPUT_PROTOCOL,
                ptr::null_mut(),
                &raw mut interface,
            )
        }
        .ok()?;
        let gop = NonNull::new(interface.cast::<GraphicsOutput>()).ok_or(Status::NOT_FOUND)?;
        // SAFETY: the firmware's own protocol, its mode and the mode's
        // information stay valid until the mode changes, and Meridian does
        // not change it while reading.
        let (mode, info) = unsafe {
            let mode = gop.as_ref().mode.as_ref().ok_or(Status::UNSUPPORTED)?;
            (mode, mode.info.as_ref().ok_or(Status::UNSUPPORTED)?)
        };
        let fb = Framebuffer {
            base: mode.frame_buffer_base,
            size: mode.frame_buffer_size as u64,
            width: info.horizontal_resolution,
            height: info.vertical_resolution,
            stride: info.pixels_per_scan_line,
            format: PixelFormat::from_uefi(info.pixel_format).ok_or(Status::UNSUPPORTED)?,
        };
        if fb.base == 0 || !fb.is_consistent() {
            return Err(Status::VOLUME_CORRUPTED);
        }
        Ok(fb)
    }

    /// The framebuffer's pixels, one `u32` each, row after row with
    /// `stride` pixels per row.
    ///
    /// # Errors
    ///
    /// `ACCESS_DENIED` while another [`Pixels`] is out, otherwise like
    /// [`System::framebuffer`].
    pub fn pixels(&self) -> Result<Pixels<'_>, Status> {
        if self.pixels_out.get() {
            return Err(Status::ACCESS_DENIED);
        }
        let fb = self.framebuffer()?;
        let len = usize::try_from(fb.pixel_count()).map_err(|_| Status::UNSUPPORTED)?;
        // SAFETY: the firmware reported this memory as the framebuffer,
        // large enough for every row (checked in framebuffer), identity
        // mapped while boot services run, and pixels_out keeps a second
        // slice from being made while this one lives.
        let slice = unsafe { core::slice::from_raw_parts_mut(fb.base as *mut u32, len) };
        self.pixels_out.set(true);
        Ok(Pixels {
            fb,
            slice,
            system: self,
        })
    }
}

/// The only way to write the framebuffer while it is out.
pub struct Pixels<'a> {
    pub fb: Framebuffer,
    slice: &'a mut [u32],
    system: &'a System,
}

impl Pixels<'_> {
    #[must_use]
    pub fn as_slice(&self) -> &[u32] {
        self.slice
    }

    pub fn as_mut_slice(&mut self) -> &mut [u32] {
        self.slice
    }
}

impl Drop for Pixels<'_> {
    fn drop(&mut self) {
        self.system.pixels_out.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn protocol_layout() {
        assert_eq!(offset_of!(GraphicsOutput, set_mode), 8);
        assert_eq!(offset_of!(GraphicsOutput, blt), 16);
        assert_eq!(offset_of!(GraphicsOutput, mode), 24);
        assert_eq!(size_of::<GraphicsOutput>(), 32);
    }

    #[test]
    fn mode_layout() {
        let offsets = [
            (offset_of!(Mode, mode), 4),
            (offset_of!(Mode, info), 8),
            (offset_of!(Mode, size_of_info), 16),
            (offset_of!(Mode, frame_buffer_base), 24),
            (offset_of!(Mode, frame_buffer_size), 32),
        ];
        for (got, want) in offsets {
            assert_eq!(got, want);
        }
        assert_eq!(size_of::<Mode>(), 40);
    }

    #[test]
    fn mode_info_layout() {
        assert_eq!(offset_of!(ModeInfo, horizontal_resolution), 4);
        assert_eq!(offset_of!(ModeInfo, vertical_resolution), 8);
        assert_eq!(offset_of!(ModeInfo, pixel_format), 12);
        assert_eq!(offset_of!(ModeInfo, pixel_information), 16);
        assert_eq!(offset_of!(ModeInfo, pixels_per_scan_line), 32);
        assert_eq!(size_of::<ModeInfo>(), 36);
    }

    #[test]
    fn pixel_formats() {
        assert_eq!(PixelFormat::from_uefi(0), Some(PixelFormat::Rgb));
        assert_eq!(PixelFormat::from_uefi(1), Some(PixelFormat::Bgr));
        assert_eq!(PixelFormat::from_uefi(2), None, "bit masks");
        assert_eq!(PixelFormat::from_uefi(3), None, "Blt only");
        assert_eq!(PixelFormat::from_uefi(u32::MAX), None);
    }

    #[test]
    fn pixels_end_at_the_last_visible_one() {
        let fb = Framebuffer {
            base: 0x8000_0000,
            size: 0,
            width: 3,
            height: 2,
            stride: 5,
            format: PixelFormat::Rgb,
        };
        assert_eq!(fb.pixel_count(), 8);
        assert_eq!(Framebuffer { height: 1, ..fb }.pixel_count(), 3);
        assert_eq!(Framebuffer { height: 0, ..fb }.pixel_count(), 0);
        let big = Framebuffer {
            width: u32::MAX,
            height: u32::MAX,
            stride: u32::MAX,
            ..fb
        };
        assert_eq!(big.pixel_count(), u64::from(u32::MAX) * u64::from(u32::MAX));
    }

    #[test]
    fn consistency() {
        let fb = Framebuffer {
            base: 0x8000_0000,
            size: 1280 * 800 * 4,
            width: 1280,
            height: 800,
            stride: 1280,
            format: PixelFormat::Bgr,
        };
        assert!(fb.is_consistent());
        assert!(
            !Framebuffer {
                size: fb.size - 1,
                ..fb
            }
            .is_consistent()
        );
        assert!(!Framebuffer { stride: 1279, ..fb }.is_consistent());
        assert!(!Framebuffer { width: 0, ..fb }.is_consistent());
        assert!(!Framebuffer { height: 0, ..fb }.is_consistent());
        let padded = Framebuffer {
            stride: 1344,
            size: 1344 * 800 * 4,
            ..fb
        };
        assert!(padded.is_consistent());
        assert!(
            !Framebuffer {
                size: fb.size,
                ..padded
            }
            .is_consistent()
        );
        let unpadded_end = (1344 * 799 + 1280) * 4;
        let short_last_row = Framebuffer {
            size: unpadded_end,
            ..padded
        };
        assert!(short_last_row.is_consistent());
        assert!(
            !Framebuffer {
                size: unpadded_end - 1,
                ..padded
            }
            .is_consistent()
        );
        let huge = Framebuffer {
            stride: u32::MAX,
            height: u32::MAX,
            width: 1,
            size: u64::MAX,
            ..fb
        };
        assert!(!huge.is_consistent(), "no overflow");
    }
}
