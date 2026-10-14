# Changelog

What changed for someone using Meridian or its tools. Every entry also says
what was checked and how, under "Verified". Versions follow SemVer and stay
at 0.x until the native API is stable.

## Unreleased

### Added

- **Repository**
  - The MIT and Apache-2.0 licences.
  - Decision records 0001 to 0012.
  - CI on GitHub Actions: lint, the test ratio, formatting, clippy, the Go and Rust tests and the QEMU test image on every push; sextant on macOS and Windows on `main` and weekly.
  - The docs: README, roadmap, architecture, testing, contributing and security.
- **`.meri`, Meridian's settings format**
  - The specification.
  - A conformance suite of 134 cases: 18 valid documents with their canonical dumps, and 116 invalid ones with their expected errors.
- **sextant**
  - `help`, and `version` with the git revision it was built from.
  - `stats`, which counts code and test lines per component. `--check` fails below 1.5 test lines per code line and says how many are missing. Ratios are cut to two decimals, never rounded up.
  - `lint`, which rejects em and en dashes, leftover markers in comments, `todo!()` and `unimplemented!()`, `unsafe` outside `raw` modules, and outside code or files without an accepted decision record.
  - A `.meri` reader, with typed getters and layering of a local file over the shared one.
  - GPT, FAT32 and ISO 9660 writers that need no outside tools and give the same bytes for the same inputs on any machine.
  - Settings from `sextant.meri`, with `sextant.local.meri` for one machine: build directory, QEMU memory, CPUs, accelerator, firmware, timeout and how long the firmware may stay silent.
  - `build`, `image`, `run` and `test`. Images are stamped with the commit, so a commit always gives the same image. `build` puts the loader and the kernel on the EFI partition. `test` boots the QEMU test image, presses the keys it asks for, keeps pictures of the screens it asks for as PNG files, and starts the machine again when the firmware hangs before printing anything.
  - Scripts for a run, whose steps wait for a line, pause, press keys and keep the screen.
  - Every image carries a loader signed with Authenticode, by the key `sign.key` names or by a development key sextant makes once per machine. Every image also carries a `boot.meri` with the defaults written out.
  - `mutate`, which changes Go and Rust code one mutant at a time in private copies of the repository and runs the tests on each, so tests that cannot fail are found. Known equivalent mutants are listed with a reason in `mutants.meri`.
- **`lib/uefi`**
  - Status codes with their names and plain-English meanings.
  - GUIDs, the memory map reader, and the UCS-2 console encoder.
  - The system table with its validation, and the firmware tables, written from UEFI 2.10. Only slots Meridian calls have a function type.
  - Pages and the memory map, files and folders on the boot volume, the framebuffer through graphics output, keys without waiting, configuration tables such as ACPI's, device paths, and loading and starting other EFI applications.
  - Firmware variables, read, written and deleted, and restarting the machine.
- **The loader (`boot`)**
  - V shows the log, everything the loader wrote, and F restarts into the firmware's settings where the firmware can do that.
  - Settings in `\meridian\boot.meri`: the countdown, the system that starts, by name or the one started last, and whether the log comes first.
  - Banner with the firmware vendor and UEFI version, COM1 logging.
  - The firmware memory map turned into kernel regions (`lib/bootinfo`), with usable memory losing every overlap with any other kind.
  - A QEMU test image that runs every crate's checks against real firmware and ends QEMU with its verdict.
  - Loading the kernel: the ELF file is checked and relocated (`lib/elf`), the kernel gets its own page tables with a direct map of memory (`lib/paging`), and the handover (`lib/bootinfo`) carries the memory map, the framebuffer and the ACPI root pointer.
  - A boot screen with a card for each system, a countdown, and the arrow keys and Enter.
  - Other systems on the boot partition, such as Windows Boot Manager, GRUB and systemd-boot, are found and started from their cards.
  - The firmware's own boot options are offered as cards too, for systems on other disks. An option the loader cannot load from its path is left to the firmware through `BootNext` and a restart.
  - An `.sbat` section, which current shims require.
- **The kernel**
  - Its own GDT, task state and IDT. Every processor exception is shown in plain words, and double faults run on a stack of their own, so running out of stack still ends on the panic screen.
  - A status screen with the processor, its cores, how long the machine has been up, kept current, and the memory map as a bar.
  - A panic screen in plain English with a QR code of the crash summary.
  - Cores from the ACPI MADT, and the time stamp counter measured against the ACPI timer.
- **Libraries**
  - `lib/font`: TrueType fonts parsed and rasterized without allocating, with Inter and JetBrains Mono under the SIL Open Font License (decision record 0012).
  - `lib/canvas`: rectangles and text on a framebuffer in either pixel order.
  - `lib/qr`: QR codes of every version and error correction level, without allocating.
  - `lib/acpi`: the root pointer, the XSDT, the MADT and the FADT, with their checksums.
  - `lib/meri`: `.meri` documents read in Rust without a heap, event by event or setting by setting, for the loader's settings.
  - `lib/x86`: the serial port, CPUID, control and model specific registers, the descriptor tables, the time stamp counter and I/O ports.

### Fixed

- sextant cleans absolute folders from its settings, so a folder written with forward slashes on Windows, or with `..` in it, matches the same folder found another way.
- The kernel no longer reads the task state through an empty task register, which Ubuntu's OVMF leaves behind.

### Verified

- **Test lines.** 26568 lines of tests for 15002 lines of code, a ratio of 1.77, and every component at 1.5 or more.
- **sextant.** All tests pass with the race detector, at 6826 lines of code and 12153 lines of tests, a ratio of 1.78, on Linux, macOS and Windows.
- **Signatures.** OpenSSL verifies the loader's signature, its signed digest and its certificate, and the digest matches a second reading of the file through Go's `debug/pe`. OVMF with Secure Boot on starts the loader signed with an enrolled key and refuses it unsigned, signed with another key, or signed over a wrong digest.
- **`lib/uefi`.** 97 host tests pass at a ratio of 1.53. Every field offset of the firmware tables is checked against the specification.
- **Mutation testing.** `sextant mutate` finds no surviving mutant in sextant; a full run takes about an hour and a half. New Rust code is mutated by hand as it is written, and every mutant was killed, or the code was simplified until the mutant disappeared. Two equivalent mutants are listed in `mutants.meri`.
- **`lib/qr`.** Every version at every level gives the same modules as `qrencode` with the same mask, and `zbarimg` reads every version back.
- **Disk images.** The writers' output is read back by `sfdisk`, `fdisk`, `fsck.fat`, `fatlabel`, `mdir`, `mcopy`, `xorriso` and `bsdtar`. Mutation testing found two real bugs, both fixed:
  - the FAT size calculation overflowed near 2 TiB
  - the ISO tree dropped a file when a folder whose name differed only in case sorted first
- **The Rust `.meri` reader** passes the whole conformance suite, and 52,000 generated documents read the same as with sextant's Go reader. `sextant mutate` finds no surviving mutant in it.
- **sextant's `.meri` reader.**
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
- **Images boot.** A GPT disk and an ISO written by sextant both boot the loader on OVMF.
- **On real firmware.** `sextant test --qemu` runs 169 checks in QEMU with KVM: the loader's and every crate's under the firmware, firmware variables, the boot screen, its log and its settings, then the kernel's on what the loader handed over, the ACPI tables, the measured counter, its screens, five exceptions caused on purpose, a panic and a stack overflow.
- **The keys and settings on a real machine.** Go tests press F on OVMF and see its setup after the restart, and boot the plain image with `verbose` and `default = example` and see the log, the countdown and the example starting.
- **What a person sees.** The boot, log, status, panic and double fault screens are kept as PNG files. A Go test checks their colours and reads the QR codes with `zbarimg`, and on the plain image checks that only the uptime changes in 2.5 seconds.
- **Another firmware.** CI runs the test image on Ubuntu's OVMF, which found a bug the development PC's OVMF hid: the kernel read the task state through an empty task register. It is fixed.
- **No machine is left behind.** A test that counts open files before and after two boots, and a test that kills the process that started QEMU and checks QEMU dies with it.

### Not built yet

Memory management, interrupts, scheduling, drivers, the file system, the
desktop and the Win32 personality: everything that makes the kernel more
than a status screen. See [TODO.md](TODO.md).
