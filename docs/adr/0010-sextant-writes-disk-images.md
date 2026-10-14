# 0010. Write disk images with sextant itself

- Status: accepted
- Date: 2026-10-14

## Context

Meridian boots from a GPT disk image whose EFI System Partition is FAT32, and from an ISO image for virtual machines and CDs. On Linux these are usually built with a chain of tools: `truncate` or `dd` for the file, `sgdisk` or `parted` for the partition table, `losetup`, `mkfs.fat` and `mount` (as root) or `mtools` for the file system, and `xorriso` or `grub-mkrescue` for the ISO.

That chain does not fit Meridian:

- sextant has to run on Linux, macOS and Windows. Meridian is built on more than one machine, and none of them may be special. Windows has none of these tools, macOS ships different ones (`hdiutil`, `newfs_msdos`), and `losetup` and `mount` need root.
- The tools differ between distributions and versions: `mtools` changes its geometry checks, `xorriso` its option set.
- They write the current time, random volume serials and random GUIDs into every image, so two builds of the same commit never match.

## Decision

sextant writes every image itself, with nothing but Go's standard library:

- `internal/disk/gpt`: the protective MBR and both copies of the GUID partition table with their CRC32s, partitions on 1 MiB boundaries.
- `internal/disk/fat`: FAT32 as described in Microsoft's fatgen103, long names included, written straight into the partition's place in the image file.
- `internal/disk/iso`: ISO 9660 at interchange level 2 with an El Torito boot catalog. Its single entry has the UEFI platform ID 0xEF and points at the FAT image, which is always written last.

**Reproducible** means the same inputs give the same bytes, on any machine:

- Every GUID is derived from a seed the caller passes in, through SHA-256.
- Every FAT and ISO timestamp comes from one time the caller passes in.
- Files are laid out in sorted order, contiguously.
- No random volume serials.

Free space is not cleared, so callers write into a fresh file, which is zero-filled and stays sparse.

The external tools still have a job, but only in the tests, as independent readers of what sextant wrote: `fsck.fat`, `fatlabel`, `mdir` and `mcopy`, `sfdisk` and `fdisk`, `xorriso` and `bsdtar`. On a machine without them those checks are skipped with a message naming the tool. With `SEXTANT_REQUIRE_TOOLS=1`, which CI on Linux sets, a missing tool fails the test instead.

## Alternatives considered

- **mtools.** Portable C and needs no root, but it is another program to install everywhere, Windows included, and its checks differ between versions.
- **losetup, mkfs.fat and mount.** Linux only, and needs root for every build.
- **grub-mkrescue or xorriso for the ISO.** `grub-mkrescue` puts GRUB on the image, and Meridian has its own bootloader. Neither tool exists on Windows.
- **dd.** Copies bytes, but builds no partition table or file system.
- **A container with all the tools.** Turns Docker into a build requirement and hides the work inside an image.

## Consequences

- About 1160 lines of Go to maintain (without comments and blank lines), covered by about 2550 lines of tests and by mutation testing.
- Images built on different machines can be compared by their checksum.
- Not built: exFAT, NTFS, an ISO that also boots when written to a USB stick, BIOS boot, and the Rock Ridge and Joliet extensions. Names on the CD therefore appear in their upper-case ISO 9660 form.
- The El Torito sector count is 16 bits wide, so an embedded partition over 32 MiB gets a count of 0. EDK2 reads that as "the rest of the disc", which is why the partition comes last. So far only `xorriso`, `bsdtar` and `fsck.fat` have read such an image; the first boot in QEMU with OVMF is the real check.
