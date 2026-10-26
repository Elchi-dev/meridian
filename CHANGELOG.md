# Changelog

What changed for someone using Meridian or its tools. Every entry also says
what was checked and how, under "Verified". Versions follow SemVer and stay
at 0.x until the native API is stable.

## Unreleased

### Added

- **Repository**
  - The MIT and Apache-2.0 licences.
  - Decision records 0001 to 0013.
  - CI on GitHub Actions: `sextant ci` on every push, then mutation testing of what the push changed; sextant on macOS and Windows on `main` and weekly.
  - The docs: README, roadmap, architecture, testing, contributing and security.
- **`.meri`, Meridian's settings format**
  - The specification.
  - A conformance suite of 134 cases: 18 valid documents with their canonical dumps, and 116 invalid ones with their expected errors.
- **sextant**
  - `help`, and `version` with the git revision it was built from.
  - `adr new "title"`, which starts the next decision record from the template, proposed and dated today, and adds its row to the index.
  - `doctor`, which checks Go, rustup and the pinned Rust toolchain, QEMU, OVMF, access to KVM and the tools the tests check sextant's work with, says how to fix what is missing, and installs it with the machine's package manager after one "y".
  - `ci`, every check CI runs in its order: the project rules, the test lines, Go and Rust formatting, clippy for every target, `go vet`, the Go tests with the race detector, the Rust tests and the QEMU test image. CI's linux job runs `sextant ci` itself.
  - `stats`, which counts code and test lines per component. `--check` fails below 1.5 test lines per code line and says how many are missing. Ratios are cut to two decimals, never rounded up.
  - `lint`, which rejects em and en dashes, leftover markers in comments, `todo!()` and `unimplemented!()`, `unsafe` outside `raw` modules, and outside code or files without an accepted decision record. A file under `third-party/` also has to match the SHA-256 its record names.
  - A `.meri` reader, with typed getters and layering of a local file over the shared one.
  - GPT, FAT32 and ISO 9660 writers that need no outside tools and give the same bytes for the same inputs on any machine.
  - Settings from `sextant.meri`, with `sextant.local.meri` for one machine: build directory, QEMU memory, CPUs, accelerator, firmware, timeout and how long the firmware may stay silent.
  - `build`, `image`, `run` and `test`. Images are stamped with the commit, so a commit always gives the same image. `build` puts the loader and the kernel on the EFI partition. `test` boots the QEMU test image, presses the keys it asks for, keeps pictures of the screens it asks for as PNG files, and starts the machine again when the firmware hangs before printing anything. `run --for` and `run --until "text"` stop the machine after a time or once the text appears, for a quick check that an image boots.
  - `key new`, which makes the release key with its certificate, never over an existing file. The release certificate is published in `keys/release.cer`, and SECURITY.md names its SHA-256.
  - Scripts for a run, whose steps wait for a line, pause, press keys and keep the screen.
  - Every image carries a loader signed with Authenticode, by the key `sign.key` names or by a development key sextant makes once per machine. Every image also carries a `boot.meri` with the defaults written out.
  - `mutate`, which changes Go and Rust code one mutant at a time in private copies of the repository and runs the tests on each, so tests that cannot fail are found. A mutant in a crate with QEMU checks that its host tests miss is tried on real firmware too. A mutant whose tests run three times as long as without it plus 30 seconds, or on Linux hold three times the memory and at least 1 GiB, is stopped and counts as killed. Known equivalent mutants are listed with a reason in `mutants.meri`.
  - On an ISO, files whose names ISO 9660 cannot hold are only on the EFI system partition inside it, which is all the firmware reads.
  - The writes that enroll a key in a firmware's Secure Boot databases from Setup Mode: db, KEK and PK as authenticated variables, signed with a detached PKCS #7 signature.
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
  - Secure Boot without firmware settings (decision record 0013): images start through Ubuntu's shim, signed by Microsoft, with the loader next to it as `grubx64.efi`. On the first boot the shim opens MokManager, where the person enrolls `meridian.cer` from the top of the disk once; after that every boot is silent.
  - No flicker into the kernel: the boot screen stays on the screen until the kernel's first frame, drawn in room the loader set aside, replaces it whole.
- **The kernel**
  - Its own GDT, task state and IDT. Every processor exception is shown in plain words, and double faults run on a stack of their own, so running out of stack still ends on the panic screen.
  - A status screen with the processor, its cores and how many of them run, how long the machine has been up, kept current, and the memory map as a bar.
  - A panic screen in plain English with a QR code of the crash summary.
  - Cores from the ACPI MADT, and the time stamp counter measured against the ACPI timer, or against the HPET where that is missing.
  - A physical frame allocator built from the memory map, and a 4 MiB heap from its frames, so `Box`, `Vec` and `String` work in the kernel. Both sit behind spin locks.
  - The local APIC in x2APIC mode, or in xAPIC mode through its page mapped uncached on processors without x2APIC, its timer measured against the time stamp counter and interrupting a hundred times a second, so the processor sleeps with `hlt` between ticks instead of spinning.
  - Every core starts: each one listed in the MADT gets INIT and two startup interrupts, runs a trampoline below 640 KiB from real mode through protected mode into long mode, and comes into the kernel on its own stack and the kernel's descriptor tables. Each core has a task state and a fault stack of its own, and its stacks are mapped with nothing below them, so one that runs over faults instead of writing into the next.
  - Kernel threads on every core: 32 priority levels kept by `lib/sched`, a run queue and an idle thread for each core, each thread on a stack of its own with nothing below it, switched when one gives way, sleeps or ends, and at every timer tick when a thread of the same or a higher priority is ready, and an idle thread that halts the processor until the next tick. The status screen's uptime sleeps between looks.
  - Ring 3: user code and data segments, system calls through `int 0x2e`, a kernel stack for every thread in the task state, and the first program, which says hello from ring 3 through a system call. Every program has an address space of its own, with its code read only and nothing run from its stack, and gives it back whole when it ends. A fault in ring 3 ends only the program.
  - Channels between programs: a program reaches kernel objects only through handles in its own table, with rights and generations, and can make a channel, send through it, receive from it and close it. Two programs on different cores talk through one, and a message can hand a handle over to the program that takes it.
  - The other cores take jobs: each has a mailbox for a function and what it works on, is woken by an interrupt when a job comes, and sleeps in between. A core only counts as running once it answered its first job with its own APIC id.
  - Pages of its own: the kernel maps and unmaps 4 KiB pages in the address space it runs in, with new page tables from the frame allocator, and makes the processor forget an unmapped page at once.
  - Threads move between cores: a core with nothing to run takes a waiting thread from a busy one, unless that thread was started for its core. A thread's FPU and SSE registers survive every switch, kept with XSAVE, AVX and AVX-512 included, where the processor has it, and with FXSAVE otherwise.
  - The kernel lies at a random 2 MiB boundary in the top 2 GiB, chosen with RDRAND. Every core runs with SMEP and SMAP: the kernel reaches a program's pages only through copies that check them first, and a copy stops short at a page another thread of the program gave back meanwhile.
  - Programs from files: `.mexec` files, ELF with Meridian's version and name notes, from `meridian/programs` on the boot partition, each page mapped with the rights its segment gives it.
  - Processes: SPAWN starts a program by its name or its path and hands it a handle, WAIT gets its exit code or the exception that ended it, and TERMINATE ends it at its next way back to ring 3, also while it waits. THREAD and END_THREAD give a program more threads; a page one of them unmaps is forgotten on every core that runs the program, through a shootdown interrupt. MAP and UNMAP give memory from a region that hands every address out once.
  - Security tokens (decision record 0014): SIDs, privileges and integrity levels, SYSTEM for what the kernel starts and a restricted copy through SPAWN. OPEN asks the access check for every file.
  - Channels carry handles: one handed over leaves its sender, needs the right to be handed over, and one nobody takes is closed with its channel.
  - Disks: PCI through ECAM, every function on every bus and behind bridges; SATA disks through AHCI, NVMe drives and virtio block devices, read and written by polling, up to sixteen frames a command on AHCI and NVMe and six on virtio; and every disk's GPT.
  - Files: FAT32 read, and a VFS with one tree, paths compared in capitals, mounts, and OPEN, READ, INFO and LIST. The partition the loader came from is at `/System/Boot`.
  - The PS/2 keyboard through the I/O APIC, its keys named by their USB HID usage, and a text console for programs, WRITE and KEY, in rows of JetBrains Mono that wrap and scroll.
  - init starts the services `meridian/services.meri` lists, each in a thread of its own, and starts one again when it ends, waiting longer each time it ended soon. A first shell runs on the console: a line edited with shift and backspace, `help`, `echo`, `ls`, `cat`, `clear` and `exit`, and any program by its name in `/System/Boot/meridian/programs` or by its path, saying how it ended.
- **Libraries**
  - `lib/font`: TrueType fonts parsed and rasterized without allocating, with Inter and JetBrains Mono under the SIL Open Font License (decision record 0012).
  - `lib/canvas`: rectangles and text on a framebuffer in either pixel order.
  - `lib/qr`: QR codes of every version and error correction level, without allocating.
  - `lib/sched`: the run queue of 32 priority levels; `lib/handles`: handle tables with rights and generations; `lib/channel`: two queues of messages between two ends.
  - `lib/frames`: a bitmap of free units and the physical frame allocator built on it; `lib/heap`: the kernel's heap in blocks of the same bitmap, handed out as offsets.
  - `lib/acpi`: the root pointer, the XSDT, the MADT and the FADT, with their checksums.
  - `lib/meri`: `.meri` documents read in Rust without a heap, event by event or setting by setting, for the loader's settings.
  - `lib/x86`: the serial port, CPUID, control and model specific registers, the descriptor tables, the time stamp counter and I/O ports.
  - `lib/abi`: the system calls programs and the kernel agree on; `lib/program`: what a program in Rust is built on, the calls as functions, `main!`, a heap and `log!`; `lib/security`: SIDs, tokens and the access check, in the shape [MS-DTYP] gives them.
  - `lib/pci`, `lib/ahci`, `lib/nvme`, `lib/virtio` and `lib/ps2`: the parts of each driver that need no hardware, from the specifications.
  - `lib/gpt`: partition tables read and written, with their copies and the protective MBR; `lib/fat`: FAT32 read, a run of sectors at a time, and written, long names with their short ones, and volumes made as fatgen103 lays them out.

### Fixed

- sextant cleans absolute folders from its settings, so a folder written with forward slashes on Windows, or with `..` in it, matches the same folder found another way.
- The kernel no longer reads the task state through an empty task register, which Ubuntu's OVMF leaves behind.

### Verified

- **Test lines.** 47356 lines of tests for 26984 lines of code, a ratio of 1.75, and every component at 1.5 or more.
- **sextant.** All tests pass with the race detector, at 7712 lines of code and 13743 lines of tests, a ratio of 1.78, on Linux, macOS and Windows.
- **Signatures.** OpenSSL verifies the loader's signature, its signed digest and its certificate, and the digest matches a second reading of the file through Go's `debug/pe`. OVMF with Secure Boot on starts the loader signed with an enrolled key and refuses it unsigned, signed with another key, or signed over a wrong digest. OVMF in Setup Mode takes Meridian's own key through the authenticated variable writes sextant makes, refuses a write whose signature was changed, and afterwards starts only what that key signed. OVMF with Microsoft's keys, as a PC from a shop has them, starts the shim, which refuses the loader until a Go test has pressed through enrolling `meridian.cer` in MokManager, and starts it after the restart; with another key's certificate enrolled it keeps refusing.
- **`lib/uefi`.** 111 host tests pass at a ratio of 1.60. Every field offset of the firmware tables is checked against the specification.
- **Mutation testing.** A run over every Rust crate, 2183 mutants with the QEMU checks for `boot`, `kernel`, `lib/uefi` and `lib/x86`, leaves none surviving, and each file changed after that run was mutated again on its own with the same result. Each survivor along the way got a test, or simpler code with nothing equivalent left to change. sextant has no surviving mutant either; a full run of it takes about an hour and a half. Sixteen equivalent mutants, fourteen in Rust and two in Go, are listed in `mutants.meri` with the reason each cannot change behaviour.
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
- **Images boot.** A GPT disk and an ISO written by sextant both boot through the shim into the loader on OVMF.
- **On real firmware.** `sextant test --qemu` runs 344 checks in QEMU with KVM, twice: on QEMU's processor, and on one without x2APIC, XSAVE or PS/2 whose NVMe and virtio disks write to copies, so the kernel's other paths are checked too: the loader's and every crate's under the firmware, firmware variables and boot options, the boot screen, its log and its settings, a firmware boot option started from its card, then the kernel's on what the loader handed over, the ACPI tables, the measured counter, its frames and heap, pages it maps and unmaps, its timer, the other cores starting, their guarded stacks and the jobs they take, its screens, five exceptions caused on purpose, a panic and a stack overflow, and then threads moving between cores, programs in ring 3 with their system calls, their threads and shootdowns, channels, tokens, the disks, the files and the console. Without KVM the time stamp counter and the ACPI timer drift apart, so one check fails there.
- **Boot options on another disk.** A Go test gives the test image a second disk: an option with the whole path to a loader there starts and comes back, and one with the short path Windows uses goes through `BootNext` and a restart, after which the firmware starts it from the second disk.
- **The keys and settings on a real machine.** Go tests press F on OVMF and see its setup after the restart, and boot the plain image with `verbose` and `default = example` and see the log, the countdown and the example starting.
- **What a person sees.** The boot, log, status, panic and double fault screens are kept as PNG files. A Go test checks their colours and reads the QR codes with `zbarimg`, and on the plain image checks that only the uptime changes in 2.5 seconds.
- **Another firmware.** CI runs the test image on Ubuntu's OVMF, which found a bug the development PC's OVMF hid: the kernel read the task state through an empty task register. It is fixed.
- **No machine is left behind.** A test that counts open files before and after two boots, and a test that kills the process that started QEMU and checks QEMU dies with it.

### Not built yet

Strata, writing files through the VFS, an installer, USB, the desktop
and the Win32 personality. See [TODO.md](TODO.md).
