//! Systems the firmware lists among its boot options, offered as cards
//! next to the ones found on the EFI partition. They may sit on another
//! disk, where the partition scan cannot see them.

use core::fmt::Write;

use canvas::Text;
use uefi::device_path;
use uefi::load_option::LoadOption;

use crate::screen::Kind;

/// A file path the way the loader compares them: `/` between folders and
/// ASCII letters in lower case, from the root of the partition.
pub type Key = Text<160>;

/// `file` as a [`Key`], or `None` when it does not fit.
pub fn key(file: impl Iterator<Item = char>) -> Option<Key> {
    let mut out = Key::new();
    for c in file {
        let c = if c == '\\' {
            '/'
        } else {
            c.to_ascii_lowercase()
        };
        out.write_char(c).ok()?;
    }
    Some(out)
}

/// What becomes of one boot option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It gets a card.
    Offered,
    /// The firmware started this loader with it.
    ThisLoader,
    /// It is inactive, hidden or an application, such as the setup.
    NotOffered,
    /// It names a whole disk or a firmware application, not a file.
    NoFile,
    /// It names a file on this partition that has a card already.
    Known,
}

/// Whether `path`, a boot option's device path, leads to the device this
/// loader sits on, whose path is `own`: both name the same GPT or MBR
/// partition, or, where one names none, as on a CD, `path` up to its file
/// is `own`.
#[must_use]
pub fn on_own_device(path: &[u8], own: &[u8]) -> bool {
    if let (Some(a), Some(b)) = (device_path::partition(path), device_path::partition(own)) {
        return a == b;
    }
    let (Some(theirs), Some(ours)) = (device_path::walk(path), device_path::walk(own)) else {
        return false;
    };
    theirs
        .take_while(|n| !(n.kind == 4 && n.subtype == 4))
        .eq(ours)
}

/// Decides about boot option `number`. `current` is the option the
/// firmware started this loader with, `own` the device path of the
/// partition or CD this loader sits on, and `known` says whether a file
/// there has a card already, given as a [`Key`].
pub fn verdict(
    option: &LoadOption<'_>,
    number: u16,
    current: Option<u16>,
    own: Option<&[u8]>,
    known: impl Fn(&str) -> bool,
) -> Verdict {
    if current == Some(number) {
        return Verdict::ThisLoader;
    }
    if !option.is_offered() {
        return Verdict::NotOffered;
    }
    let Some(file) = device_path::file(option.path).and_then(key) else {
        return Verdict::NoFile;
    };
    let here = own.is_some_and(|own| on_own_device(option.path, own));
    if here && known(file.as_str()) {
        return Verdict::Known;
    }
    Verdict::Offered
}

/// Whether `file`, a [`Key`] on this partition, has a card already: it is
/// the shim at the firmware's fallback path or this loader next to it, or
/// one of `scanned`, the loaders that have cards so far, written as they
/// were found. Two boot options for the same file so make one card.
pub fn known_here<'s>(file: &str, mut scanned: impl Iterator<Item = &'s str>) -> bool {
    let is = |path: &str| key(path.chars()).is_some_and(|k| k.as_str() == file);
    is(crate::FIRMWARE_PATH) || is(crate::LOADER_PATH) || scanned.any(is)
}

/// Whether a boot option the loader failed to load with `status` should be
/// left to the firmware, which can follow paths the loader cannot, such as
/// the short ones Windows writes that start at the partition.
#[must_use]
pub fn needs_the_firmware(status: uefi::Status) -> bool {
    matches!(
        status,
        uefi::Status::NOT_FOUND | uefi::Status::INVALID_PARAMETER
    )
}

/// The kind of system a loader belongs to, by its file, given as a [`Key`].
#[must_use]
pub fn kind(file: &str) -> Kind {
    let name = file.rsplit('/').next().unwrap_or(file);
    match name {
        "bootmgfw.efi" => Kind::Windows,
        "shimx64.efi" | "grubx64.efi" | "systemd-bootx64.efi" => Kind::Linux,
        _ => Kind::Other,
    }
}

/// The name on the card: Windows for Windows, as the partition scan names
/// it, else the option's description, or its file when it has none.
#[must_use]
pub fn name(option: &LoadOption<'_>, file: &str) -> Text<48> {
    let mut out = Text::new();
    if kind(file) == Kind::Windows {
        let _ = out.write_str("Windows");
    } else if option.description().next().is_some() {
        for c in option.description() {
            if out.write_char(c).is_err() {
                break;
            }
        }
    } else {
        let _ = out.write_str(file.rsplit('/').next().unwrap_or(file));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: [u8; 16] = [0x5a; 16];

    fn own() -> Vec<u8> {
        path(OWN, None)
    }

    /// A device path: a hard drive node with `signature`, then `file`.
    fn path(signature: [u8; 16], file: Option<&str>) -> Vec<u8> {
        let mut p = vec![4, 1, 42, 0];
        let mut data = [0u8; 38];
        data[20..36].copy_from_slice(&signature);
        p.extend(data);
        if let Some(file) = file {
            let units: Vec<u16> = file.encode_utf16().chain([0]).collect();
            p.extend([4, 4]);
            p.extend(u16::try_from(4 + 2 * units.len()).unwrap().to_le_bytes());
            p.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        }
        p.extend([0x7f, 0xff, 4, 0]);
        p
    }

    /// A device path that is only a file, with no partition before it.
    fn path_without_partition(file: &str) -> Vec<u8> {
        let units: Vec<u16> = file.encode_utf16().chain([0]).collect();
        let mut p = vec![4, 4];
        p.extend(u16::try_from(4 + 2 * units.len()).unwrap().to_le_bytes());
        p.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        p.extend([0x7f, 0xff, 4, 0]);
        p
    }

    fn option(attributes: u32, description: &str, path: &[u8]) -> Vec<u8> {
        let mut d = attributes.to_le_bytes().to_vec();
        d.extend(u16::try_from(path.len()).unwrap().to_le_bytes());
        d.extend(
            description
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes),
        );
        d.extend(path);
        d
    }

    fn judge(data: &[u8], number: u16, known: &[&str]) -> Verdict {
        let o = LoadOption::parse(data).unwrap();
        verdict(&o, number, Some(1), Some(&own()), |f| known.contains(&f))
    }

    #[test]
    fn paths_compare_without_case_or_backslashes() {
        let k = key("\\EFI\\Microsoft\\Boot\\BOOTMGFW.EFI".chars()).unwrap();
        assert_eq!(k.as_str(), "/efi/microsoft/boot/bootmgfw.efi");
        assert_eq!(
            key("Ä/x".chars()).unwrap().as_str(),
            "Ä/x",
            "only ASCII changes case"
        );
        assert!(
            key("a".repeat(161).chars()).is_none(),
            "too long to compare"
        );
        assert_eq!(
            key("a".repeat(160).chars()).map(|k| k.as_str().len()),
            Some(160)
        );
    }

    #[test]
    fn a_windows_option_on_another_disk_gets_a_card() {
        let windows = option(
            1,
            "Windows Boot Manager",
            &path([7; 16], Some("\\EFI\\Microsoft\\Boot\\bootmgfw.efi")),
        );
        assert_eq!(
            judge(&windows, 3, &["/efi/microsoft/boot/bootmgfw.efi"]),
            Verdict::Offered,
            "the same file on another disk is another system"
        );
        assert_eq!(judge(&windows, 1, &[]), Verdict::ThisLoader);
    }

    #[test]
    fn options_for_files_this_partition_shows_already_get_none() {
        let here = option(1, "ubuntu", &path(OWN, Some("\\EFI\\ubuntu\\shimx64.efi")));
        assert_eq!(
            judge(&here, 4, &["/efi/ubuntu/shimx64.efi"]),
            Verdict::Known
        );
        assert_eq!(
            judge(&here, 4, &["/efi/ubuntu/grubx64.efi"]),
            Verdict::Offered,
            "another file on this partition"
        );
        let o = LoadOption::parse(&here).unwrap();
        assert_eq!(
            verdict(&o, 4, None, None, |_| true),
            Verdict::Offered,
            "without knowing its own partition the loader hides nothing"
        );
        let nowhere = option(1, "loose file", &path_without_partition("\\EFI\\loose.efi"));
        let o = LoadOption::parse(&nowhere).unwrap();
        assert_eq!(
            verdict(&o, 9, None, None, |_| true),
            Verdict::Offered,
            "no partition on either side is not the same partition"
        );
    }

    #[test]
    fn disks_applications_and_inactive_options_get_none() {
        let disk = option(1, "UEFI QEMU HARDDISK", &path([7; 16], None));
        assert_eq!(judge(&disk, 5, &[]), Verdict::NoFile);
        let setup = option(1 | 0x100, "UiApp", &path([7; 16], Some("x")));
        assert_eq!(judge(&setup, 6, &[]), Verdict::NotOffered);
        let off = option(0, "old", &path([7; 16], Some("\\a.efi")));
        assert_eq!(judge(&off, 7, &[]), Verdict::NotOffered);
        let hidden = option(1 | 8, "hidden", &path([7; 16], Some("\\a.efi")));
        assert_eq!(judge(&hidden, 8, &[]), Verdict::NotOffered);
    }

    #[test]
    fn the_file_says_the_kind() {
        assert_eq!(kind("/efi/microsoft/boot/bootmgfw.efi"), Kind::Windows);
        assert_eq!(kind("/efi/ubuntu/shimx64.efi"), Kind::Linux);
        assert_eq!(kind("/efi/fedora/grubx64.efi"), Kind::Linux);
        assert_eq!(kind("/efi/systemd/systemd-bootx64.efi"), Kind::Linux);
        assert_eq!(kind("/efi/other/tool.efi"), Kind::Other);
        assert_eq!(kind("bootmgfw.efi"), Kind::Windows, "a file at the root");
        assert_eq!(kind("/efi/x/notbootmgfw.efi"), Kind::Other);
    }

    #[test]
    fn cards_are_named_by_the_description() {
        let named = |description: &str, file: &str| {
            let data = option(1, description, &path([7; 16], Some(file)));
            name(&LoadOption::parse(&data).unwrap(), file)
                .as_str()
                .to_owned()
        };
        assert_eq!(
            named("Windows Boot Manager", "/efi/microsoft/boot/bootmgfw.efi"),
            "Windows"
        );
        assert_eq!(named("Fedora", "/efi/fedora/shimx64.efi"), "Fedora");
        assert_eq!(named("", "/efi/other/tool.efi"), "tool.efi");
        let long = "a very long description that goes on and on past the card";
        assert_eq!(
            named(long, "/x.efi"),
            &long[..48],
            "cut to what a card holds"
        );
    }

    #[test]
    fn this_loader_and_scanned_loaders_are_known() {
        let scanned = [
            "/EFI/example/grubx64.efi",
            "/EFI/Microsoft/Boot/bootmgfw.efi",
        ];
        assert!(known_here("/efi/boot/bootx64.efi", scanned.into_iter()));
        assert!(known_here("/efi/example/grubx64.efi", scanned.into_iter()));
        assert!(known_here(
            "/efi/microsoft/boot/bootmgfw.efi",
            scanned.into_iter()
        ));
        assert!(!known_here("/efi/other/tool.efi", scanned.into_iter()));
        assert!(known_here("/efi/boot/bootx64.efi", [].into_iter()));
        assert!(known_here("/efi/boot/grubx64.efi", [].into_iter()));
        assert!(!known_here("/efi/boot/mmx64.efi", [].into_iter()));
        assert!(!known_here("/efi/example/grubx64.efi", [].into_iter()));
    }

    #[test]
    fn only_paths_the_loader_cannot_follow_go_to_the_firmware() {
        use uefi::Status;
        assert!(needs_the_firmware(Status::NOT_FOUND));
        assert!(needs_the_firmware(Status::INVALID_PARAMETER));
        assert!(
            !needs_the_firmware(Status::SECURITY_VIOLATION),
            "Secure Boot refused it; a restart would not help"
        );
        assert!(!needs_the_firmware(Status::LOAD_ERROR));
        assert!(!needs_the_firmware(Status::OUT_OF_RESOURCES));
    }

    /// A CD's device path: PCI, SATA, then a CD-ROM node, with no partition.
    fn cd(file: Option<&str>) -> Vec<u8> {
        let mut p = vec![2, 1, 12, 0, 0xd0, 0x41, 3, 0x0a, 0, 0, 0, 0];
        p.extend([3, 0x12, 10, 0, 1, 0, 0xff, 0xff, 0, 0]);
        p.extend([4, 2, 24, 0]);
        p.extend([0; 20]);
        if let Some(file) = file {
            let units: Vec<u16> = file.encode_utf16().chain([0]).collect();
            p.extend([4, 4]);
            p.extend(u16::try_from(4 + 2 * units.len()).unwrap().to_le_bytes());
            p.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        }
        p.extend([0x7f, 0xff, 4, 0]);
        p
    }

    #[test]
    fn partitions_compare_by_signature() {
        assert!(on_own_device(&path(OWN, Some("\\a.efi")), &own()));
        assert!(!on_own_device(&path([7; 16], Some("\\a.efi")), &own()));
        assert!(
            !on_own_device(&path(OWN, Some("\\a.efi")), &cd(None)),
            "a partition is not the CD"
        );
    }

    #[test]
    fn without_partitions_the_whole_path_up_to_the_file_counts() {
        assert!(on_own_device(&cd(Some("\\EFI\\x.efi")), &cd(None)));
        let mut other_cd = cd(Some("\\EFI\\x.efi"));
        other_cd[16] = 2;
        assert!(!on_own_device(&other_cd, &cd(None)), "another SATA port");
        assert!(
            !on_own_device(&cd(Some("\\x.efi")), &[1, 2]),
            "a broken own path"
        );
        assert!(!on_own_device(&[1, 2], &cd(None)));
    }

    #[test]
    fn options_on_a_cd_this_loader_sits_on_are_known_too() {
        let data = option(1, "scanned", &cd(Some("\\EFI\\example\\grubx64.efi")));
        let o = LoadOption::parse(&data).unwrap();
        let known = |f: &str| f == "/efi/example/grubx64.efi";
        assert_eq!(verdict(&o, 3, None, Some(&cd(None)), known), Verdict::Known);
        assert_eq!(
            verdict(&o, 3, None, Some(&own()), known),
            Verdict::Offered,
            "booted from a disk, the CD is elsewhere"
        );
    }
}
