# TODO

Open work, in milestone order. This file is the only issue tracker:
GitHub issues are off. Why the milestones come in this order is in
[docs/ROADMAP.md](docs/ROADMAP.md).

## M0: Foundation

Repository:

- [x] Licences (MIT or Apache-2.0), `.gitignore`, `.gitattributes`
- [x] `AI_DISCLAIMER.md`
- [x] Decision records 0001 to 0006, 0009 and 0010, with index and template
- [x] Decision records 0007 (assembly inside Rust) and 0008 (unsafe in raw modules), committed with the loader
- [x] README, roadmap, architecture, testing, TODO, changelog, contributing, security
- [x] CI workflow in `.github/workflows/ci.yml`: lint, host tests, QEMU tests with KVM, `stats --check`; sextant built and tested on macOS and Windows too
- [x] Decision record 0011 for `actions/checkout`, the only outside action, pinned to a commit hash
- [x] Ruleset on `main`: CI must be green, no force pushes
- [x] `mutants.meri` with the known equivalent mutants
- [x] Mutation of changed files in CI (`sextant mutate --since` the commit before the push)

sextant:

- [x] `help`, `version` with the git revision
- [x] `stats` with `--check`, `--files`, `--min`
- [x] `lint`: dashes, markers, stubs, unsafe outside raw modules, outside code without a record
- [x] The `.meri` reader, specification and 134-case conformance suite
- [x] GPT, FAT32 and ISO writers, reproducible
- [x] Settings from `sextant.meri` and `sextant.local.meri`; `stats` takes `min-ratio` from them
- [x] `stats` counts `unsafe` blocks, functions and impls, not `unsafe extern fn` pointer types
- [x] `mutate` for Rust and Go, each mutant in a private copy of the repository
- [x] `mutate` for code only QEMU tests reach: crates with a `qemu-test` feature boot the test image for mutants their host tests miss
- [x] `doctor`, with installing after one "y"
- [x] `build`
- [x] `image` (GPT disk image and ISO), stamped with the commit
- [x] `run`, in a window or headless, `--debug` waits for GDB
- [x] `test --host` and `test --qemu`, reading the serial protocol and exit codes 33 and 35, answering `PRESS` and `SCREEN` lines, starting the machine again when the firmware stalls
- [x] `ci`, which CI's linux job runs
- [x] `adr new`
- [ ] `release`, `flash`, `debug`

Loader and UEFI:

- [x] `lib/uefi`: status codes, GUIDs, memory map, console encoder, system table, checked against UEFI 2.10
- [x] `boot`: banner, memory map conversion where usable memory loses every overlap, QEMU firmware and keyboard checks
- [x] `lib/uefi`: pages, the memory map, files on the boot volume, configuration tables, leaving boot services
- [x] `lib/x86`: the serial port, CPUID, EFER and CR0, shared by loader and kernel
- [ ] Find out why OVMF under KVM hangs before printing anything in about one boot in four on the development PC, while TCG never does; `qemu.stall` and restarts work around it for now

## M1: Boot

- [x] TrueType rasterizer in `lib/font`, Inter and JetBrains Mono under one decision record (0012)
- [x] Graphical boot screen with OS cards, arrow keys, Enter, countdown, drawn off screen and copied whole
- [x] `V` for the full log, `F` for firmware settings on the boot screen
- [x] Neutral icon for the Windows card (a W on a sky blue tile), no Microsoft logo
- [x] Detect other systems by a scan of the EFI partition
- [x] Detect other systems from `BootOrder` and `Boot####`, for loaders outside the EFI partition
- [x] A QEMU test with a second disk, for a boot option on another drive and one with a short path that goes through `BootNext`
- [x] Start other systems through `LoadImage` and `StartImage`, and come back to the boot screen if they return
- [ ] Start Windows and Linux on the laptop, with Secure Boot on
- [x] Rust `.meri` reader passing the conformance suite (`lib/meri`), without allocating
- [x] `boot.meri`: the loader reads its settings with `lib/meri`
- [ ] Shim under `third-party/` with its decision record
- [x] `.sbat` section in `meridian.efi`
- [x] Authenticode signer in sextant; development keys per machine
- [ ] The release key as a CI secret, which Samuel adds
- [x] Secure Boot test in QEMU with OVMF's Secure Boot build and enrolled keys (Ubuntu's snakeoil test key)
- [x] Enroll Meridian's own development key into a test variable store, so the Secure Boot test needs no distribution's template
- [x] Memory regions for the handover (`lib/bootinfo`)
- [x] Kernel ELF loading (`lib/elf`), higher-half page tables (`lib/paging`), the handover structure, the jump
- [x] Framebuffer in the handover
- [x] The boot screen kept on the framebuffer until the kernel draws, without flicker
- [x] Kernel: entry point, serial log, QEMU checks of the handover
- [x] Kernel: framebuffer text (`lib/canvas` with `lib/font`)
- [x] Kernel: GDT, TSS with a fault stack, IDT, every exception caught and shown on the panic screen
- [x] Status screen: processor, free memory, the memory map as a bar
- [x] Status screen: CPU cores from the ACPI MADT, uptime from the time stamp counter measured against the ACPI timer, updated live
- [x] Panic screen in plain English with a QR code of the crash summary (`lib/qr`), checked by a QEMU test and read back with `zbarimg`
- [x] sextant keeps the test images' screens as PNG files (`SCREEN <name>`)
- [ ] Booted on the laptop with Secure Boot on, Windows and Arch Linux still starting from the menu

## M2: Memory and CPUs

- [x] Physical frame allocator and kernel heap (`lib/frames`, `lib/heap`)
- [x] Virtual memory: the kernel maps and unmaps pages of its own, on the tables CR3 holds, with `invlpg` after an unmap
- [x] The local APIC in x2APIC mode, its timer measured against the TSC, interrupts a hundred times a second and `hlt` in between
- [ ] The local APIC in xAPIC mode, for processors without x2APIC, and HPET; it needs the APIC's page mapped uncached. QEMU without KVM has no x2APIC, so there the kernel runs on one core and spins, which the test image checks
- [ ] Under QEMU without KVM the time stamp counter and the ACPI timer drift apart, so `kernel.tsc.agrees` fails there
- [x] All cores started: INIT and startup through the x2APIC, a trampoline below 640 KiB, each core on its own stack and the kernel's tables
- [x] The other cores get task states of their own, with fault stacks
- [x] The other cores' stacks are mapped on their own with an unmapped page below, so one that runs over faults
- [ ] The other cores get work to do
- [ ] ACPI tables parsed

## M3: Processes

- [ ] Scheduler with 32 priority levels
- [ ] User mode and system calls
- [ ] Objects, handles with access masks, security tokens
- [ ] `.mexec` loader (ELF with Meridian note sections)
- [ ] IPC channels that carry handles
- [ ] KASLR

## M4: Storage and Strata

- [ ] virtio-blk, AHCI, NVMe
- [ ] GPT and FAT32 in the kernel
- [ ] Strata: copy-on-write, checksums, snapshots, case-preserving names, instant locate index, reserved room for transactions, deduplication and expiry
- [ ] VFS
- [ ] No writes to partitions Meridian did not create

## M5: Userland

- [ ] init and the service manager
- [ ] The shell, with an `sh` mode; `cls` and `clear`
- [ ] Command-line tools
- [ ] Text terminal

## M6: Windows programs, step 1

- [ ] PE loader for 64-bit and 32-bit programs
- [ ] ntdll, kernel32, the C runtime parts console programs need
- [ ] Registry store
- [ ] The stop message for missing functions; `exe-check`
- [ ] Test programs in `tests/win32/` and reference outputs from GitHub's Windows runners

## M7: Horizon

- [ ] CPU compositor with damage tracking; virtio-gpu
- [ ] PS/2, xHCI and USB HID
- [ ] Taskbar, launcher, overview, workspaces, notification and control centres, widgets
- [ ] Terminal, Files, Settings, System monitor, Image viewer, Clock and calendar
- [ ] Declarative UI toolkit
- [ ] Keyboard layouts from CLDR, with a decision record
- [ ] Reserved CPU time and memory for compositor and cursor

## M8: Windows programs, step 2

- [ ] user32 and gdi32
- [ ] Standard controls in Meridian's look
- [ ] Who lock
- [ ] Installer undo
- [ ] PuTTY and 7-Zip

## Waiting

- [ ] The repository is made public (Samuel); then private vulnerability reporting is switched on
