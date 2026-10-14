//! The parts of the Meridian bootloader that do not need the firmware to
//! run, so they are tested on any machine. The UEFI application in
//! `main.rs` puts them together with the firmware calls.

#![cfg_attr(not(test), no_std)]

pub mod handover;
pub mod log;
pub mod memory;
pub mod menu;
pub mod screen;
pub mod settings;
pub mod systems;

use core::fmt::{self, Write};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the kernel is on the EFI system partition.
pub const KERNEL_PATH: &str = "/meridian/kernel";

/// Where the firmware finds this loader when no boot entry names it.
pub const LOADER_PATH: &str = "/EFI/BOOT/BOOTX64.EFI";

/// The loader's SBAT data, which the shim reads from its `.sbat` section
/// before it starts it, so a generation of the loader with a security hole
/// can be refused: the format line, then Meridian's own. The fields are
/// component, generation, vendor, package, version and URL, as rhboot's
/// SBAT.md describes them.
pub const SBAT: &str = concat!(
    "sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md\n",
    "meridian,1,Meridian,meridian,",
    env!("CARGO_PKG_VERSION"),
    ",https://github.com/Elchi-dev/meridian\n",
);

/// [`SBAT`] as bytes of a fixed size, for the section.
///
/// # Panics
///
/// When `N` is not the length of the text, which fails the build for the
/// section's constant.
#[must_use]
pub const fn sbat_bytes<const N: usize>() -> [u8; N] {
    let text = SBAT.as_bytes();
    assert!(text.len() == N, "N is the length of the SBAT text");
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        out[i] = text[i];
        i += 1;
    }
    out
}

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

/// The log's lines about the machine: the screen's size, Secure Boot from
/// the `SecureBoot` variable, and whether F can open the firmware's
/// settings.
///
/// # Errors
///
/// Fails when the writer does.
pub fn describe(
    out: &mut impl Write,
    screen: Option<(u32, u32)>,
    secure_boot_variable: Result<&[u8], uefi::Status>,
    firmware_settings: bool,
) -> fmt::Result {
    match screen {
        Some((width, height)) => writeln!(out, "Screen: {width} x {height}")?,
        None => writeln!(out, "Screen: none, so there is no boot screen")?,
    }
    writeln!(out, "Secure Boot: {}", secure_boot(secure_boot_variable))?;
    if firmware_settings {
        writeln!(out, "Firmware settings: F opens them")
    } else {
        writeln!(
            out,
            "Firmware settings: this firmware cannot be asked to open them"
        )
    }
}

/// What the log says about Secure Boot, from the `SecureBoot` variable.
#[must_use]
pub fn secure_boot(value: Result<&[u8], uefi::Status>) -> &'static str {
    match value {
        Ok([1]) => "on",
        Ok([0]) => "off",
        Err(uefi::Status::NOT_FOUND) => "off, this firmware does not have it",
        _ => "unknown, the firmware's answer made no sense",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbat_has_the_format_line_and_meridians_own() {
        let lines: Vec<Vec<&str>> = SBAT.lines().map(|l| l.split(',').collect()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            [
                "sbat",
                "1",
                "SBAT Version",
                "sbat",
                "1",
                "https://github.com/rhboot/shim/blob/main/SBAT.md"
            ]
        );
        assert_eq!(
            lines[1],
            [
                "meridian",
                "1",
                "Meridian",
                "meridian",
                VERSION,
                "https://github.com/Elchi-dev/meridian"
            ]
        );
        assert!(SBAT.ends_with('\n'), "every record ends its line");
        assert!(SBAT.is_ascii());
    }

    #[test]
    fn sbat_bytes_are_the_text() {
        const N: usize = SBAT.len();
        assert_eq!(&sbat_bytes::<N>(), SBAT.as_bytes());
    }

    #[test]
    #[should_panic(expected = "N is the length")]
    fn sbat_bytes_need_the_exact_length() {
        let _ = sbat_bytes::<4>();
    }

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

    #[test]
    fn the_machine_is_described() {
        let mut s = String::new();
        describe(&mut s, Some((1280, 800)), Ok(&[1]), true).unwrap();
        assert_eq!(
            s,
            "Screen: 1280 x 800\nSecure Boot: on\nFirmware settings: F opens them\n"
        );
        s.clear();
        describe(&mut s, None, Err(uefi::Status::NOT_FOUND), false).unwrap();
        assert_eq!(
            s,
            "Screen: none, so there is no boot screen\n\
             Secure Boot: off, this firmware does not have it\n\
             Firmware settings: this firmware cannot be asked to open them\n"
        );
        assert!(describe(&mut Broken, None, Ok(&[0]), true).is_err());
    }

    #[test]
    fn secure_boot_is_said_in_words() {
        assert_eq!(secure_boot(Ok(&[1])), "on");
        assert_eq!(secure_boot(Ok(&[0])), "off");
        assert_eq!(
            secure_boot(Err(uefi::Status::NOT_FOUND)),
            "off, this firmware does not have it"
        );
        let nonsense = "unknown, the firmware's answer made no sense";
        assert_eq!(secure_boot(Ok(&[2])), nonsense);
        assert_eq!(secure_boot(Ok(&[1, 0])), nonsense);
        assert_eq!(secure_boot(Ok(&[])), nonsense);
        assert_eq!(secure_boot(Err(uefi::Status::DEVICE_ERROR)), nonsense);
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
