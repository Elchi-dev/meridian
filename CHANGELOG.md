# Changelog

What changed for someone using Meridian or its tools. Every entry also says
what was checked and how, under "Verified". Versions follow SemVer and stay
at 0.x until the native API is stable.

## Unreleased

### Added

- **Repository**
  - The MIT and Apache-2.0 licences.
  - Decision records 0001 to 0011.
  - CI on GitHub Actions: lint, the test ratio, formatting, clippy, the Go and Rust tests and the QEMU test image on every push; sextant on macOS and Windows on `main` and weekly.
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
  - Settings from `sextant.meri`, with `sextant.local.meri` for one machine: build directory, QEMU memory, CPUs, accelerator, firmware, timeout and how long the firmware may stay silent.
  - `build`, `image`, `run` and `test`. Images are stamped with the commit, so a commit always gives the same image. `test` boots the QEMU test image, presses the keys it asks for, and starts the machine again when the firmware hangs before printing anything.
- **`lib/uefi`**
  - Status codes with their names and plain-English meanings.
  - GUIDs, the memory map reader, and the UCS-2 console encoder.
  - The system table with its validation, and the firmware tables, written from UEFI 2.10. Only slots Meridian calls have a function type.
- **The loader (`boot`)**
  - Banner with the firmware vendor and UEFI version, COM1 logging.
  - The firmware memory map turned into kernel regions (`lib/bootinfo`), with usable memory losing every overlap with any other kind.
  - A QEMU test image that runs every crate's checks against real firmware and ends QEMU with its verdict.

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

  - Enter and the down arrow, pressed by sextant through the QEMU monitor
- **Images boot.** A GPT disk and an ISO written by sextant both boot the loader on OVMF; `sextant test --qemu` runs all ten firmware checks in a few seconds.
- **No machine is left behind.** A test that counts open files before and after two boots, and a test that kills the process that started QEMU and checks QEMU dies with it.

### Not built yet

The kernel, and with it everything a person would call an operating system.
See [TODO.md](TODO.md).
