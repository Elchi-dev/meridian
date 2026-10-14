//! The parts of the Meridian bootloader that do not need the firmware to
//! run, so they are tested on any machine. The UEFI application in
//! `main.rs` puts them together with the firmware calls.

#![cfg_attr(not(test), no_std)]

pub mod memory;

use core::fmt::{self, Write};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Text for the person in front of the machine. It goes to the firmware
/// console, and to the serial port only when there is no console: firmware
/// such as OVMF already mirrors its console to the serial port, and writing
/// to both would print every line twice.
pub struct Out<'a, S: Write> {
    pub screen: Option<uefi::raw::Console<'a>>,
    pub serial: &'a mut S,
}

impl<S: Write> Write for Out<'_, S> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        match self.screen.as_mut() {
            Some(screen) => screen.write_str(s),
            None => self.serial.write_str(s),
        }
    }
}

/// Splits a UEFI revision into its parts: the high half is the major
/// version, the low half the minor version times ten plus the sub-minor,
/// so 2.70 is `(2 << 16) + 70` and 2.3.1 is `(2 << 16) + 31`.
#[must_use]
pub fn uefi_version(revision: u32) -> (u32, u32, u32) {
    let minor = revision & 0xffff;
    (revision >> 16, minor / 10, minor % 10)
}

/// The first lines the loader prints: its version and the firmware it runs on.
///
/// # Errors
///
/// Fails when the writer does.
pub fn banner(
    out: &mut impl Write,
    vendor: impl Iterator<Item = char>,
    firmware_revision: u32,
    uefi_revision: u32,
) -> fmt::Result {
    writeln!(out, "Meridian boot {VERSION}")?;
    out.write_str("Firmware: ")?;
    for c in vendor {
        out.write_char(c)?;
    }
    let (major, minor, sub) = uefi_version(uefi_revision);
    write!(
        out,
        " (revision {firmware_revision:#x}), UEFI {major}.{minor}"
    )?;
    if sub != 0 {
        write!(out, ".{sub}")?;
    }
    out.write_char('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parts() {
        assert_eq!(uefi_version((2 << 16) + 70), (2, 7, 0));
        assert_eq!(uefi_version((2 << 16) + 100), (2, 10, 0));
        assert_eq!(uefi_version((2 << 16) + 31), (2, 3, 1));
        assert_eq!(uefi_version((1 << 16) + 10), (1, 1, 0));
        assert_eq!(uefi_version(0xffff_ffff), (0xffff, 6553, 5));
    }

    #[test]
    fn banner_names_loader_firmware_and_uefi_version() {
        let mut s = String::new();
        banner(&mut s, "EDK II".chars(), 0x10000, (2 << 16) + 70).unwrap();
        assert_eq!(
            s,
            format!("Meridian boot {VERSION}\nFirmware: EDK II (revision 0x10000), UEFI 2.7\n")
        );
    }

    #[test]
    fn banner_shows_a_sub_minor_version() {
        let mut s = String::new();
        banner(
            &mut s,
            "American Megatrends".chars(),
            0x5_0019,
            (2 << 16) + 31,
        )
        .unwrap();
        assert!(
            s.ends_with("Firmware: American Megatrends (revision 0x50019), UEFI 2.3.1\n"),
            "{s}"
        );
    }

    #[test]
    fn banner_without_vendor() {
        let mut s = String::new();
        banner(&mut s, "".chars(), 0, (2 << 16) + 100).unwrap();
        assert!(s.ends_with("Firmware:  (revision 0x0), UEFI 2.10\n"), "{s}");
    }

    struct Broken;

    impl Write for Broken {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn serial_still_gets_the_text_when_there_is_no_screen() {
        let mut serial = String::new();
        let mut out = Out {
            screen: None,
            serial: &mut serial,
        };
        out.write_str("hello\n").unwrap();
        assert_eq!(serial, "hello\n");
    }

    #[test]
    fn a_failing_serial_port_is_reported() {
        let mut broken = Broken;
        let mut out = Out {
            screen: None,
            serial: &mut broken,
        };
        assert!(out.write_str("x").is_err());
        assert!(banner(&mut Broken, "x".chars(), 0, 0).is_err());
    }
}
