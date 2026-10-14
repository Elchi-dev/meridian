# Changelog

What changed for someone using Meridian or its tools. Every entry also says
what was checked and how, under "Verified". Versions follow SemVer and stay
at 0.x until the native API is stable.

## Unreleased

### Added

- **Repository**
  - The MIT and Apache-2.0 licences.
  - Decision records 0001 to 0006, 0009 and 0010.
  - The docs: README, roadmap, architecture, testing, contributing and security.
- **`.meri`, Meridian's settings format**
  - The specification.
  - A conformance suite of 131 cases: 18 valid documents with their canonical dumps, and 113 invalid ones with their expected errors.
- **sextant**
  - `help`, and `version` with the git revision it was built from.
  - `stats`, which counts code and test lines per component. `--check` fails below 1.5 test lines per code line and says how many are missing. Ratios are cut to two decimals, never rounded up.
  - `lint`, which rejects em and en dashes, leftover markers in comments, `todo!()` and `unimplemented!()`, `unsafe` outside `raw` modules, and outside code or files without an accepted decision record.
  - A `.meri` reader, with typed getters and layering of a local file over the shared one.
  - GPT, FAT32 and ISO 9660 writers that need no outside tools and give the same bytes for the same inputs on any machine.
- **`lib/uefi`**
  - Status codes with their names and plain-English meanings.
  - GUIDs, the memory map reader, and the UCS-2 console encoder.
  - The system table with its validation, and the firmware tables, written from UEFI 2.10.

### Verified

- **sextant.** All tests pass with the race detector, at 3873 lines of code and 6643 lines of tests, a ratio of 1.71.
- **`lib/uefi`.** 52 host tests pass at a ratio of 1.51. Every field offset of the firmware tables is checked against the specification.
- **Mutation testing**, so far by hand per package: every mutant was killed, or the code was simplified until the mutant disappeared. Three equivalent mutants are known and will be listed in `mutants.meri`.
- **Disk images.** The writers' output is read back by `sfdisk`, `fdisk`, `fsck.fat`, `fatlabel`, `mdir`, `mcopy`, `xorriso` and `bsdtar`. Mutation testing found two real bugs, both fixed:
  - the FAT size calculation overflowed near 2 TiB
  - the ISO tree dropped a file when a folder whose name differed only in case sorted first
- **`.meri` reader.**
  - It passes the whole conformance suite.
  - Fuzzing ran 14 million inputs without a failure, after it had found an ordering bug in layering, which is fixed.
  - It reads 113.8 MB/s on the development PC.
- **The firmware calls of `lib/uefi`** pass their checks against OVMF (edk2 202608) in QEMU 11.1 with KVM:
  - the vendor name
  - the table revision
  - `Stall`
  - clearing the console
  - writing a probe longer than one encoder piece, which arrived whole on the serial mirror
  - a missing glyph being only a warning

  These checks were run by hand with the loader's test image, which is committed together with the loader.

### Not built yet

The kernel, and with it everything a person would call an operating system.
See [TODO.md](TODO.md).
