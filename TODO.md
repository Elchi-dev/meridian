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
- [x] Decision record 0013: Ubuntu's shim signed by Microsoft, MokManager for enrolling Meridian's key
- [x] Shim and MokManager under `third-party/shim/`, `sextant image` writing them, and a QEMU test on OVMF with Microsoft's keys that enrolls the key through MokManager
- [x] `.sbat` section in `meridian.efi`
- [x] Authenticode signer in sextant; development keys per machine
- [x] The release key as a CI secret, which Samuel added (`keys/release.cer`)
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
- [x] The local APIC in xAPIC mode, for processors without x2APIC, through its page mapped uncached; the test image runs a second time on a processor without x2APIC
- [x] HPET: found in its ACPI table, mapped uncached, its main counter running; the time stamp counter is measured against it where the ACPI timer is missing
- [ ] Under QEMU without KVM the time stamp counter and the ACPI timer drift apart, so `kernel.tsc.agrees` fails there
- [x] All cores started: INIT and startup through the local APIC, a trampoline below 640 KiB, each core on its own stack and the kernel's tables
- [x] The other cores get task states of their own, with fault stacks
- [x] The other cores' stacks are mapped on their own with an unmapped page below, so one that runs over faults
- [x] The other cores get work to do: a mailbox each, woken by an interrupt, asleep in between
- [x] ACPI tables parsed: the root pointer, the XSDT, the MADT, the FADT's power management timer and the HPET table (`lib/acpi`), no AML yet

## M3: Processes

- [x] Kernel threads on the first core: 32 priority levels (`lib/sched`), a switch when one gives way, sleeping on the timer, an idle thread
- [x] Preemption: at every tick the timer hands the processor to a ready thread of the same or a higher priority, and wakes the sleepers; spin locks keep interrupts off while held
- [x] Threads on every core: a run queue and an idle thread each, every core's timer ticking, a thread for another core waking it
- [x] Threads move between cores: a core with nothing to run takes a waiting thread from a busy one, unless it was started for its core
- [x] Each thread's FPU and SSE registers kept across switches, before programs use them: XSAVE with AVX and AVX-512 where the processor has them, FXSAVE otherwise, fresh for every new thread
- [x] Checks that measure time hold on a busy host: `kernel.thread.freed_elsewhere`, `kernel.smp.quick`, `kernel.tsc.agrees` and `kernel.timer.ticks` failed now and then while other machines ran beside them, and other cores now get a second to come
- [x] Threads in programs, and with them TLB shootdowns: a page one thread unmaps must not stay cached on another core (THREAD, END_THREAD, the SHOOTDOWN interrupt)
- [x] TERMINATE: a process ends at its next way back to ring 3, also while it waits
- [x] Ring 3 and system calls: user segments, `int 0x2e`, a kernel stack per thread in the task state, the first program saying hello, faults in ring 3 ending only the program
- [x] An address space of its own for every program, given back whole when it ends; the kernel's half shared by all
- [x] Handles with access masks, in a table per program, with generations so a closed handle reaches nothing (`lib/handles`)
- [x] Processes as objects: SPAWN by file name with a handle handed over, WAIT for the exit code or the exception (`kernel::process`)
- [x] Security tokens: SIDs, privileges and integrity levels; SYSTEM for what the kernel starts, a restricted copy through SPAWN, TOKEN to read one (`lib/security`, ADR 0014)
- [x] The access check put to use once there are files: OPEN asks it for every file
- [ ] The access check for named objects
- [x] One lock for the serial port, so lines from threads on different cores or a preempted thread cannot run into each other
- [x] `.mexec` loader: ELF files carrying Meridian's version note, named in their name note ([format](docs/formats/mexec.md)); `main!` writes both
- [x] Channels between programs: make, send, receive, close, freed once both ends are closed (`lib/channel`)
- [x] Channels that carry handles: a handle handed over leaves its sender, needs the right to, and one nobody takes is closed with its channel
- [x] KASLR: the kernel at a random 2 MiB boundary of the top 2 GiB, from RDRAND
- [x] SMEP and SMAP on every core; the kernel touches a program's pages only inside `with_user_access`, and every way in from ring 3 clears AC and puts the kernel's stack segment back
- [x] Programs from files: the loader reads `meridian/programs` (`lib/bootinfo` version 3), the kernel maps each page of the ELF file with the rights its segments give it (`kernel::program`)
- [x] Memory for programs: MAP and UNMAP, from a heap region that hands each address out once
- [x] `lib/program`, the runtime for programs in Rust: the calls as functions, `main!`, a heap, `log!`; `hello` built by sextant and checking every call from ring 3

## M4: Storage and Strata

- [x] PCI through ECAM: every function on every bus, behind bridges too (`lib/pci`)
- [x] AHCI: SATA disks read with READ DMA EXT, by polling (`lib/ahci`)
- [x] NVMe: the first namespace read through one pair of queues, by polling (`lib/nvme`)
- [x] AHCI ports started again after a command fails
- [x] virtio-blk: version 1 over PCI, one queue, polled reads (`lib/virtio`)
- [x] Writes on AHCI, NVMe and virtio-blk, checked in QEMU on copies of the disks, with a read-only virtio disk refused
- [ ] Interrupts for disks instead of polling
- [x] Reads of more than one sector at a time: FAT32 reads a file a piece at a time, every sector that lies in one run in one read, and keeps the FAT sector it needs
- [x] Commands longer than a frame: NVMe and AHCI move up to sixteen frames per command, virtio up to six
- [x] GPT in the kernel (`lib/gpt`)
- [x] FAT32 read in the kernel (`lib/fat`)
- [x] FAT32 written: files and directories made, written and removed, with long names and their short ones (`lib/fat`)
- [x] FAT32 volumes and GPT tables made from nothing, read back by `fsck.fat`, `mtools` and `sfdisk` (`lib/fat`, `lib/gpt`)
- [ ] An installer that lays out a disk with these and copies the system on
- [ ] Strata: copy-on-write, checksums, snapshots, case-preserving names, instant locate index, reserved room for transactions, deduplication and expiry
- [x] VFS, reading: one tree, paths compared in capitals, mounts, OPEN, READ and INFO, the EFI system partition at /System/Boot (`kernel::vfs`)
- [x] VFS: listing directories (LIST), and the partition the loader came from at /System/Boot (`lib/bootinfo` version 4)
- [x] VFS: writing: `/System` on the system partition, OPEN with Windows' ways of making and emptying files, WRITE_FILE, REMOVE and MAKE_DIRECTORY, on virtio disks only until Strata (9.6)
- [x] No writes to partitions Meridian did not create: every write goes through `raw::disk`, which lets through only what `made.meri` and `unlocked.meri` on the system partition name (ADR 0015)

## M5: Userland

- [x] init: the services `meridian/services.meri` lists, each kept in a thread of its own and started again when it ends, waiting longer each time it ended soon; the kernel starts init alone
- [x] init ends after the threads that keep its services, so a service one of them started a moment before cannot outlive it
- [x] Services that run once and services that wait for others (`once`, `after` in `services.meri`)
- [x] Services asked about from the shell: init gives each program a line to ask over, and the shell's `services` lists them
- [x] A first shell on the console: a line edited with shift and backspace, `help`, `echo`, `ls`, `cat`, `clear` and `cls`, `exit`, on every image
- [x] The shell starts a program by its name in `/System/Boot/meridian/programs` or by its path (SPAWN takes paths), and says how it ended
- [ ] The shell, with an `sh` mode (ADR 0016)
- [ ] Command-line tools
- [x] A text console in the kernel: WRITE and KEY, rows of `JetBrains Mono` that wrap and scroll, a US layout in the runtime
- [ ] Text terminal as a program of its own, keyboard layouts beyond US

## M6: Windows programs, step 1

- [ ] PE loader for 64-bit and 32-bit programs
- [ ] ntdll, kernel32, the C runtime parts console programs need
- [ ] Registry store
- [ ] The stop message for missing functions; `exe-check`
- [ ] Test programs in `tests/win32/` and reference outputs from GitHub's Windows runners

## M7: Horizon

- [ ] CPU compositor with damage tracking; virtio-gpu
- [x] PS/2 keyboard through the I/O APIC, keys named by their USB HID usage (`lib/ps2`)
- [ ] PS/2 mouse, xHCI and USB HID
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
