# Architecture

How Meridian is put together. Most of this is design, not code yet; every
section says which. The pages in [architecture/](architecture/) go deeper
into the big parts, and [the decision records](adr/README.md) say why.

## From power button to desktop

```
  firmware (UEFI)
      |
  shim, signed by Microsoft's UEFI CA          planned (M1)
      |
  meridian.efi, the loader                     exists: banner and firmware checks
      |   boot menu, other systems, boot.meri
      |   loads the kernel, maps it, hands over memory map and framebuffer
      v
  kernel                                       planned (M1 to M4)
      |   memory, CPUs, scheduler, handles and objects, IPC, drivers
      v
  services in user space                       planned (M5, M7)
      |   init and services, Horizon compositor, package service
      v
  personalities                                planned (M5, M6, M8)
      |   native (.mexec)   Win32 (.exe)   later POSIX and Linux
      v
  apps
```

## The kernel model

Meridian's kernel is a **hybrid**, like NT and XNU:

- **In the kernel.** The core runs in one address space: memory, scheduling, the object manager, IPC, interrupts and the drivers. The drivers are written in Rust, so a driver bug is far less likely to corrupt kernel memory than in C.
- **In user space.** Services that do not need kernel privileges run as processes: the window compositor, the service manager, the package service.

Everything a program can hold is a **kernel object reached through a
handle** with access rights: processes, threads, memory sections, events,
mutexes, files, IPC channels. This is the model Windows programs already
expect, so the Windows personality is thin, and it is the model native
Meridian apps use as well. Details: [architecture/kernel.md](architecture/kernel.md).

Drivers for the most important hardware ship with the system, every common
network chip included, so a fresh install can always get online and fetch
the rest.

## Programs

| Kind | Format | Personality | When |
| --- | --- | --- | --- |
| Native Meridian app | `.mexec`: ELF64 with Meridian's metadata (icon, permissions, sandbox) in ELF note sections | Meridian runtime, Rust first, a C ABI later | M3 |
| Windows program | PE32+ and PE32 | Meridian's own ntdll, kernel32, user32, gdi32 and more | M6, M8 |
| POSIX program | ELF | a POSIX layer for porting C programs | later |
| Linux program | ELF | Linux system calls, like WSL 1 | later |

The Windows personality: [architecture/win32.md](architecture/win32.md).

## Files

- **The layout.** Readable names, as on macOS: `/Applications`, `/System`, `/Users`, `/Volumes`.
- **The Windows view.** Windows programs see `C:\` as `/Windows`, and `C:\Users\<name>` is the person's real home folder, so a Windows program's "Documents" is their Documents.
- **The file system.** The native file system is Strata, copy-on-write with snapshots and checksums ([architecture/strata.md](architecture/strata.md)).
- **Settings.** Every settings and system file is `.meri` ([formats/meri.md](formats/meri.md)). The Settings app writes the same files a person can edit by hand. Nobody has to.

## Security

- **From the start.** NX, SMEP, SMAP, W^X and guard pages on every stack; KASLR from M3.
- **Tokens.** Every process carries a security token from the start. The UI begins with one user; the model already supports several.
- **Sandboxing.** Optional for apps. Camera and microphone always ask.
- **Secure Boot.** It runs through a shim signed by Microsoft, then Meridian's loader, signed by sextant with Meridian's key. Checking the kernel's signature needs Meridian's own cryptography and comes later; until then the chain of trust ends at the loader.
- **Power without fear.** A person can do anything on their system without being afraid of breaking it: Strata snapshots make system changes undoable, and the system volume is protected until the person unlocks it.

## Transparency

- **Nothing hidden.** The System monitor shows every process, handle, driver and mapping.
- **Readable.** The system log is meant to be read by people.
- **Verbose boot.** A key held at boot shows every log line.
- **Documented.** Every system call is documented.
- **No telemetry.** There is none, ever.

System state is read through typed APIs and the tools built on them, not
through a `/proc` style file tree.

## The desktop

Horizon draws everything, first on the CPU into the framebuffer the
firmware hands over, later on the GPU. Details and the look:
[architecture/horizon.md](architecture/horizon.md). Text everywhere, from
the boot screen on, comes from Meridian's own TrueType rasterizer:
[architecture/fonts.md](architecture/fonts.md).

## Repository layout

```
meridian/
  boot/            UEFI loader (Rust)                                exists
  kernel/          the kernel (Rust and assembly)                    started
  lib/uefi/        UEFI types and calls                              exists
  lib/bootinfo/    what the loader hands the kernel                  exists
  lib/elf/         ELF64 reading and loading                         exists
  lib/paging/      x86_64 page tables                                exists
  lib/x86/         serial port and CPU instructions                  exists
  lib/font/        TrueType parser and rasterizer                    exists
  lib/meri/        .meri reader that needs no heap                   exists
  lib/...          more no_std crates for kernel and userland        planned
  user/            init, shell, command-line tools, terminal, apps   planned
  win32/           PE loader, ntdll, kernel32, user32, gdi32 ...     planned
  tests/meri/      .meri conformance suite                           exists
  tests/win32/     test programs and their Windows outputs           planned
  tools/sextant/   build and dev tool (Go)                           exists
  third-party/     outside files, each with a decision record        exists
  docs/            these pages, the records, the formats             exists
```

## Rules that cut across everything

| Rule | Record |
| --- | --- |
| No outside code; outside files only with a record | [0003](adr/0003-zero-dependencies.md) |
| Windows behaviour only from public documents and black-box tests | [0004](adr/0004-clean-room.md) |
| Stable Rust, pinned to one version | [0006](adr/0006-rust-stable-pinned.md) |
| Assembly inside Rust, Intel syntax | 0007 |
| `unsafe` only in `raw` modules, each block with its reason | 0008 |
| 1.5 test lines per code line, mutation testing, no mocks, no stubs | [0009](adr/0009-testing-rules.md) |
| `.meri` for settings and system files | [0005](adr/0005-meri-format.md) |
| Disk images written by sextant itself | [0010](adr/0010-sextant-writes-disk-images.md) |

## Machines

Meridian is written and tested on a desktop PC (AMD Ryzen 5 5600G, Arch
Linux) and built and checked on real hardware on a laptop that dual boots
Windows and Arch Linux with Secure Boot on. The laptop stands for any
computer: nothing in the tools is tuned to it. sextant runs on Linux, macOS
and Windows hosts.
