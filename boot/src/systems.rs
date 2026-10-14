//! Finding other systems on the EFI system partition: each folder of
//! \EFI is looked through for the loaders this list knows.

use crate::screen::Kind;

/// The loaders looked for in each folder, best first. A signed shim comes
/// before GRUB, so a system keeps starting with Secure Boot on.
const LOADERS: [(&str, Kind); 5] = [
    ("Boot/bootmgfw.efi", Kind::Windows),
    ("systemd-bootx64.efi", Kind::Linux),
    ("shimx64.efi", Kind::Linux),
    ("grubx64.efi", Kind::Linux),
    ("refind_x64.efi", Kind::Other),
];

/// Folders that hold no other system: the firmware's fallback, where this
/// loader sits, and Meridian's own.
const SKIPPED: [&str; 4] = [".", "..", "BOOT", "meridian"];

/// What the folder `folder` of \EFI holds, if anything: the kind of
/// system and the loader's file inside the folder. `exists` says whether
/// a file of the folder is there.
pub fn classify(folder: &str, exists: impl Fn(&str) -> bool) -> Option<(Kind, &'static str)> {
    if SKIPPED.iter().any(|s| s.eq_ignore_ascii_case(folder)) {
        return None;
    }
    LOADERS
        .iter()
        .find(|(file, _)| exists(file))
        .map(|&(file, kind)| (kind, file))
}

/// The names distributions are known by, by the folder they use.
const NAMES: [(&str, &str); 11] = [
    ("arch", "Arch Linux"),
    ("ubuntu", "Ubuntu"),
    ("fedora", "Fedora"),
    ("debian", "Debian"),
    ("opensuse", "openSUSE"),
    ("manjaro", "Manjaro"),
    ("pop", "Pop!_OS"),
    ("endeavouros", "EndeavourOS"),
    ("centos", "CentOS"),
    ("systemd", "Linux"),
    ("refind", "rEFInd"),
];

/// The name to show for the system in `folder`: Windows for Microsoft's
/// loader, the distribution for the folders Linux distributions use, and
/// the folder's own name otherwise.
#[must_use]
pub fn display_name(folder: &str, kind: Kind) -> &str {
    if kind == Kind::Windows {
        return "Windows";
    }
    NAMES
        .iter()
        .find(|(dir, _)| dir.eq_ignore_ascii_case(folder))
        .map_or(folder, |&(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn having<'a>(files: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |f| files.contains(&f)
    }

    #[test]
    fn windows_is_found_by_its_boot_manager() {
        let found = classify(
            "Microsoft",
            having(&["Boot/bootmgfw.efi", "Boot/memtest.efi"]),
        );
        assert_eq!(found, Some((Kind::Windows, "Boot/bootmgfw.efi")));
        assert_eq!(display_name("Microsoft", Kind::Windows), "Windows");
    }

    #[test]
    fn shim_comes_before_grub() {
        let both = classify(
            "ubuntu",
            having(&["grubx64.efi", "shimx64.efi", "mmx64.efi"]),
        );
        assert_eq!(both, Some((Kind::Linux, "shimx64.efi")));
        let grub = classify("arch", having(&["grubx64.efi"]));
        assert_eq!(grub, Some((Kind::Linux, "grubx64.efi")));
    }

    #[test]
    fn other_loaders_are_known_too() {
        assert_eq!(
            classify("systemd", having(&["systemd-bootx64.efi"])),
            Some((Kind::Linux, "systemd-bootx64.efi"))
        );
        assert_eq!(
            classify("refind", having(&["refind_x64.efi"])),
            Some((Kind::Other, "refind_x64.efi"))
        );
    }

    #[test]
    fn folders_without_a_known_loader_hold_nothing() {
        assert_eq!(classify("Dell", having(&["diags.efi"])), None);
        assert_eq!(classify("empty", having(&[])), None);
    }

    #[test]
    fn the_fallback_and_meridian_are_skipped() {
        let everything = having(&["grubx64.efi", "Boot/bootmgfw.efi"]);
        for folder in ["BOOT", "boot", "Boot", "meridian", "MERIDIAN", ".", ".."] {
            assert_eq!(classify(folder, &everything), None, "{folder}");
        }
        assert!(
            classify("bootloaders", &everything).is_some(),
            "only the exact names"
        );
    }

    #[test]
    fn distributions_get_their_names() {
        assert_eq!(display_name("arch", Kind::Linux), "Arch Linux");
        assert_eq!(display_name("ARCH", Kind::Linux), "Arch Linux");
        assert_eq!(display_name("opensuse", Kind::Linux), "openSUSE");
        assert_eq!(display_name("pop", Kind::Linux), "Pop!_OS");
        assert_eq!(display_name("refind", Kind::Other), "rEFInd");
        assert_eq!(
            display_name("gentoo", Kind::Linux),
            "gentoo",
            "an unknown folder keeps its name"
        );
        assert_eq!(
            display_name("arch", Kind::Windows),
            "Windows",
            "Windows' loader wins over the folder"
        );
    }
}
