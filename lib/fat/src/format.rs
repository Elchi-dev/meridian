//! Making a FAT32 volume, as fatgen103 lays one out: the boot sector and
//! its copy, `FSInfo` and its copy, two FATs sized as its formula gives,
//! and a root directory in the first cluster that holds only the label.

use crate::write::short_entry;
use crate::{Error, Volume};

/// The sectors before the first FAT, and where in them the copies lie.
const RESERVED: u16 = 32;
const BACKUP: u16 = 6;

/// How many sectors of 512 bytes a cluster of a volume of `total` sectors
/// holds, by fatgen103's table: up to 260 MB 1, up to 8 GB 8, up to 16 GB
/// 16, up to 32 GB 32, and 64 past that.
#[must_use]
pub fn sectors_per_cluster(total: u64) -> u8 {
    match total {
        0..=532_480 => 1,
        532_481..=16_777_216 => 8,
        16_777_217..=33_554_432 => 16,
        33_554_433..=67_108_864 => 32,
        _ => 64,
    }
}

/// How many sectors each of two FATs takes on a volume of `total`
/// sectors with clusters of `per_cluster`, by fatgen103's formula.
#[must_use]
pub fn fat_sectors(total: u32, per_cluster: u8) -> u32 {
    let left = total.saturating_sub(u32::from(RESERVED));
    let per = 128 * u32::from(per_cluster) + 1;
    left.div_ceil(per)
}

/// The boot sector of a volume of `total` sectors that lies `hidden`
/// sectors into its disk, with the serial number `id` and the label
/// `label`.
///
/// # Errors
///
/// When the volume would have too few clusters for FAT32, or more sectors
/// than 32 bits count.
pub fn boot_sector(total: u64, hidden: u32, id: u32, label: &[u8; 11]) -> Result<[u8; 512], Error> {
    let total = u32::try_from(total).map_err(|_| Error::TooLarge)?;
    let per_cluster = sectors_per_cluster(u64::from(total));
    let fat = fat_sectors(total, per_cluster);
    let mut b = [0u8; 512];
    b[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"MERIDIAN");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = per_cluster;
    b[14..16].copy_from_slice(&RESERVED.to_le_bytes());
    b[16] = 2;
    b[21] = 0xf8;
    b[24..26].copy_from_slice(&63u16.to_le_bytes());
    b[26..28].copy_from_slice(&255u16.to_le_bytes());
    b[28..32].copy_from_slice(&hidden.to_le_bytes());
    b[32..36].copy_from_slice(&total.to_le_bytes());
    b[36..40].copy_from_slice(&fat.to_le_bytes());
    b[44..48].copy_from_slice(&2u32.to_le_bytes());
    b[48..50].copy_from_slice(&1u16.to_le_bytes());
    b[50..52].copy_from_slice(&BACKUP.to_le_bytes());
    b[64] = 0x80;
    b[66] = 0x29;
    b[67..71].copy_from_slice(&id.to_le_bytes());
    b[71..82].copy_from_slice(label);
    b[82..90].copy_from_slice(b"FAT32   ");
    // No code to boot: a jump to itself, should anything start it.
    b[90..92].copy_from_slice(&[0xeb, 0xfe]);
    b[510..512].copy_from_slice(&[0x55, 0xaa]);
    // Only its size can keep a sector made so from being read as FAT32.
    Volume::read(&b).map_err(|_| Error::NotFat32)?;
    Ok(b)
}

/// The `FSInfo` sector of a volume of `clusters` clusters, all free but
/// the root directory's.
#[must_use]
pub fn fsinfo(clusters: u32) -> [u8; 512] {
    let mut f = [0u8; 512];
    f[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    f[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    f[488..492].copy_from_slice(&(clusters - 1).to_le_bytes());
    f[492..496].copy_from_slice(&3u32.to_le_bytes());
    f[508..512].copy_from_slice(&0xaa55_0000u32.to_le_bytes());
    f
}

/// Makes a FAT32 volume of `total` sectors of 512 bytes, `hidden` sectors
/// into its disk, through `write`, and answers it. Only the sectors a
/// volume reads before it holds anything are written: the reserved ones,
/// both FATs and the root directory, which holds the label.
///
/// # Errors
///
/// As for [`boot_sector`], and when a sector cannot be written.
pub fn format(
    total: u64,
    hidden: u32,
    id: u32,
    label: &[u8; 11],
    mut write: impl FnMut(u64, &[u8]) -> bool,
) -> Result<Volume, Error> {
    let boot = boot_sector(total, hidden, id, label)?;
    let volume = Volume::read(&boot)?;
    let info = fsinfo(volume.clusters);
    let zero = [0u8; 512];
    let mut put = |at: u64, bytes: &[u8]| {
        if write(at, bytes) {
            Ok(())
        } else {
            Err(Error::Write)
        }
    };
    for sector in 0..RESERVED {
        let bytes = match sector {
            0 | BACKUP => &boot,
            1 | 7 => &info,
            _ => &zero,
        };
        put(u64::from(sector), bytes)?;
    }
    let mut first = [0u8; 512];
    first[..12].copy_from_slice(&[
        0xf8, 0xff, 0xff, 0x0f, 0xff, 0xff, 0xff, 0x0f, 0xff, 0xff, 0xff, 0x0f,
    ]);
    for copy in 0..2u64 {
        let start = u64::from(RESERVED) + copy * u64::from(volume.fat_sectors);
        put(start, &first)?;
        for sector in start + 1..start + u64::from(volume.fat_sectors) {
            put(sector, &zero)?;
        }
    }
    // The root directory holds the label too, where tools look for it.
    let mut labelled = [0u8; 512];
    labelled[..32].copy_from_slice(&short_entry(label, 0x08, 0));
    let root = volume.first_sector(2).ok_or(Error::NotFat)?;
    put(root, &labelled)?;
    for sector in root + 1..root + u64::from(volume.sectors_per_cluster) {
        put(sector, &zero)?;
    }
    Ok(volume)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fat;
    use crate::write::Writer;
    use std::cell::RefCell;
    use std::process::Command;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// How many tools ran: tests run side by side, so each run gets a file
    /// of its own.
    static RUNS: AtomicUsize = AtomicUsize::new(0);

    fn require_tool(name: &str) -> bool {
        if Command::new(name).arg("--help").output().is_ok() {
            return true;
        }
        assert!(
            std::env::var("SEXTANT_REQUIRE_TOOLS").as_deref() != Ok("1"),
            "{name} is required but missing"
        );
        false
    }

    /// A file of its own to keep a volume in.
    fn scratch() -> std::path::PathBuf {
        let n = RUNS.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("meridian-format-{}-{n}.img", std::process::id()))
    }

    /// Runs `tool` with `args` on the volume `image`, and answers whether
    /// it was content and what it printed.
    fn run(tool: &str, image: &[u8], args: &[&str]) -> Option<(bool, String)> {
        if !require_tool(tool) {
            return None;
        }
        let path = scratch();
        std::fs::write(&path, image).unwrap();
        let ran = run_on(tool, &path, args);
        let _ = std::fs::remove_file(&path);
        Some(ran)
    }

    /// Runs `tool` with `args` on the volume in the file at `path`.
    fn run_on(tool: &str, path: &std::path::Path, args: &[&str]) -> (bool, String) {
        let mut command = Command::new(tool);
        for a in args {
            if *a == "IMAGE" {
                command.arg(path);
            } else {
                command.arg(a);
            }
        }
        let out = command.output().unwrap();
        let mut said = String::from_utf8_lossy(&out.stdout).into_owned();
        said.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), said)
    }

    /// A volume of `total` sectors made in memory.
    fn made(total: u64) -> Rc<RefCell<Vec<u8>>> {
        let image = Rc::new(RefCell::new(vec![
            0xa5u8;
            usize::try_from(total).unwrap() * 512
        ]));
        let into = Rc::clone(&image);
        format(
            total,
            2048,
            0x1234_5678,
            b"MERIDIAN   ",
            move |at, bytes: &[u8]| {
                let start = usize::try_from(at).unwrap() * 512;
                into.borrow_mut()[start..start + bytes.len()].copy_from_slice(bytes);
                true
            },
        )
        .unwrap();
        image
    }

    #[test]
    fn a_volume_made_is_one_fsck_finds_sound() {
        let image = made(81_920);
        let bytes = image.borrow().clone();
        if let Some((sound, said)) = run("fsck.fat", &bytes, &["-n", "IMAGE"]) {
            assert!(sound, "{said}");
        }
        if let Some((listed, out)) = run("mdir", &bytes, &["-i", "IMAGE", "::/"]) {
            assert!(
                listed && out.contains("MERIDIAN") && out.contains("No files"),
                "{out}"
            );
        }
        let v = Volume::read(&bytes[..512].try_into().unwrap()).unwrap();
        assert_eq!((v.sectors_per_cluster, v.fat_sectors, v.root), (1, 635, 2));
        assert_eq!(v.clusters, 81_920 - 32 - 2 * 635);
        assert_eq!(
            bytes[6 * 512..7 * 512],
            bytes[..512],
            "the boot sector's copy"
        );
        assert_eq!(bytes[7 * 512..8 * 512], bytes[512..1024], "and FSInfo's");
    }

    #[test]
    fn files_are_written_on_a_volume_made() {
        let image = made(81_920);
        let (from, to) = (Rc::clone(&image), Rc::clone(&image));
        let fat = Fat::open(move |at, into: &mut [u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            into.copy_from_slice(&from.borrow()[start..start + into.len()]);
            true
        })
        .unwrap();
        let mut w = Writer::new(fat, move |at, bytes: &[u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            to.borrow_mut()[start..start + bytes.len()].copy_from_slice(bytes);
            true
        });
        let (folder, _) = w.create(2, "EFI", true).unwrap();
        let (mut file, place) = w.create(folder.first, "readme.txt", false).unwrap();
        w.write_at(&mut file, 0, b"made by Meridian\n").unwrap();
        w.save(place, &file).unwrap();
        let bytes = image.borrow().clone();
        if let Some((typed, out)) = run("mtype", &bytes, &["-i", "IMAGE", "::/EFI/readme.txt"]) {
            assert!(typed && out == "made by Meridian\n", "{out}");
        }
        if let Some((sound, said)) = run("fsck.fat", &bytes, &["-n", "IMAGE"]) {
            assert!(sound, "{said}");
        }
    }

    #[test]
    fn a_volume_of_larger_clusters_is_made_whole() {
        use std::io::{Read, Seek, SeekFrom, Write};
        if !require_tool("fsck.fat") || !require_tool("mdir") {
            return;
        }
        // 300 MB, held sparse: only its first MiB is written beforehand,
        // with bytes no empty volume holds, so the root's every sector
        // has to be cleared for it to read as empty.
        let total = 600_000u64;
        let path = scratch();
        let mut file = std::fs::File::create_new(&path).unwrap();
        file.write_all(&vec![0xa5; 1 << 20]).unwrap();
        file.set_len(total * 512).unwrap();
        let file = Rc::new(RefCell::new(file));
        let into = Rc::clone(&file);
        let volume = format(total, 0, 7, b"BIGGER     ", move |at, bytes: &[u8]| {
            let mut f = into.borrow_mut();
            f.seek(SeekFrom::Start(at * 512)).is_ok() && f.write_all(bytes).is_ok()
        })
        .unwrap();
        assert_eq!((volume.sectors_per_cluster, volume.fat_sectors), (8, 586));
        let (sound, said) = run_on("fsck.fat", &path, &["-n", "IMAGE"]);
        let (listed, out) = run_on("mdir", &path, &["-i", "IMAGE", "::/"]);
        let mut root = vec![0u8; 8 * 512];
        let mut f = file.borrow_mut();
        f.seek(SeekFrom::Start((32 + 2 * 586) * 512)).unwrap();
        f.read_exact(&mut root).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(sound, "{said}");
        assert!(listed && out.contains("No files"), "{out}");
        assert_eq!(&root[..11], b"BIGGER     ");
        assert!(
            root[32..].iter().all(|&b| b == 0),
            "the rest of the root is clear"
        );
    }

    #[test]
    fn clusters_grow_with_the_volume_as_fatgen103_says() {
        let sizes = [
            (70_000, 1),
            (532_480, 1),
            (532_481, 8),
            (16_777_216, 8),
            (16_777_217, 16),
            (33_554_432, 16),
            (67_108_864, 32),
            (67_108_865, 64),
        ];
        for (total, per) in sizes {
            assert_eq!(sectors_per_cluster(total), per, "{total}");
        }
        assert_eq!(fat_sectors(81_920, 1), 635, "fatgen103's sum for 40 MiB");
        assert_eq!(fat_sectors(1_048_576, 8), 1_023, "and for 512 MiB");
    }

    #[test]
    fn a_volume_too_small_or_too_large_is_not_made() {
        assert_eq!(
            boot_sector(60_000, 0, 1, b"SMALL      "),
            Err(Error::NotFat32)
        );
        assert_eq!(boot_sector(20, 0, 1, b"TINY       "), Err(Error::NotFat32));
        assert_eq!(fat_sectors(20, 1), 0, "nothing is left for a FAT");
        assert_eq!(
            boot_sector(1 << 33, 0, 1, b"LARGE      "),
            Err(Error::TooLarge)
        );
        let refused = format(81_920, 0, 1, b"NOWHERE    ", |_, _: &[u8]| false);
        assert_eq!(refused, Err(Error::Write));
    }

    #[test]
    fn the_boot_sector_holds_what_was_asked() {
        let b = boot_sector(81_920, 2048, 0xcafe_f00d, b"MERIDIAN   ").unwrap();
        assert_eq!(&b[3..11], b"MERIDIAN");
        assert_eq!(
            u32::from_le_bytes(b[28..32].try_into().unwrap()),
            2048,
            "hidden sectors"
        );
        assert_eq!(
            u32::from_le_bytes(b[67..71].try_into().unwrap()),
            0xcafe_f00d
        );
        assert_eq!(
            (&b[71..82], &b[82..90]),
            (&b"MERIDIAN   "[..], &b"FAT32   "[..])
        );
        assert_eq!(
            (b[48], b[50], b[66], b[510], b[511]),
            (1, 6, 0x29, 0x55, 0xaa)
        );
        let f = fsinfo(80_000);
        assert_eq!(u32::from_le_bytes(f[488..492].try_into().unwrap()), 79_999);
        assert_eq!(u32::from_le_bytes(f[492..496].try_into().unwrap()), 3);
    }
}
