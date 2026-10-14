<div align="center">

```
███╗   ███╗███████╗██████╗ ██╗██████╗ ██╗ █████╗ ███╗   ██╗
████╗ ████║██╔════╝██╔══██╗██║██╔══██╗██║██╔══██╗████╗  ██║
██╔████╔██║█████╗  ██████╔╝██║██║  ██║██║███████║██╔██╗ ██║
██║╚██╔╝██║██╔══╝  ██╔══██╗██║██║  ██║██║██╔══██║██║╚██╗██║
██║ ╚═╝ ██║███████╗██║  ██║██║██████╔╝██║██║  ██║██║ ╚████║
╚═╝     ╚═╝╚══════╝╚═╝  ╚═╝╚═╝╚═════╝ ╚═╝╚═╝  ╚═╝╚═╝  ╚═══╝
```

**An operating system written from scratch that runs your Windows programs natively :3**

[![CI](https://github.com/Elchi-dev/meridian/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Elchi-dev/meridian/actions/workflows/ci.yml)
![Status](https://img.shields.io/badge/status-M1%20boot-FF8A1C)
[![Rust](https://img.shields.io/badge/Rust-1.99.0-B7410E?logo=rust)](rust-toolchain.toml)
[![Go](https://img.shields.io/badge/Go-1.27-12A4A1?logo=go)](tools/sextant/go.mod)
![Dependencies](https://img.shields.io/badge/dependencies-0-22B45E)
[![License](https://img.shields.io/badge/license-MIT%20or%20Apache--2.0-2F7CF6)](#licence)
[![Made by Elchi](https://img.shields.io/badge/made%20by-Elchi-FF8A1C)](https://github.com/Elchi-dev)

</div>

---

## Overview

I use three operating systems, and none of them is the one I want.

Windows runs everything, but it fights you. A fresh install that lacks the
network driver sends you looking for a USB cable to tether your phone. The
task manager hides half of what is going on, and "this file is in use"
never tells you by what.

macOS feels good to use, but it runs a fraction of the software, and only
on Apple's hardware.

Linux is fast and shows you everything, but you pay for that in setup
time, and one wrong command can take the whole system down.

Meridian is my attempt at the one I want:

- **Windows programs run natively.** Same instruction set, no emulation, no
  virtual machine. The CPU runs the `.exe`'s own code, and its system calls
  go straight into Meridian's kernel.
- **A desktop worth looking at.** Horizon is colourful, solid and fast, and
  every part of its look can be changed by the person using it.
- **Nothing hidden.** The system monitor shows every process, handle and
  driver. The boot can show every step. Every system call is documented.
  There is no telemetry, ever.
- **Power without fear.** The file system keeps snapshots, so a broken
  install or a wrong command can be undone with one click.

> Meridian is at the very beginning. Today there is the build tool, which
> writes and signs the boot images itself, the settings format, and a UEFI
> loader with a boot screen: a card for each system it finds, the log on
> V, the firmware settings on F, and its settings in `boot.meri`. It loads
> a higher-half kernel that catches every processor exception on a panic
> screen with a QR code, and shows the processor, its cores, the memory
> and a live uptime. **It does nothing more yet.** Everything below about
> the running system describes where Meridian is going.
> [ROADMAP](docs/ROADMAP.md) and [TODO](TODO.md) say exactly what is
> finished.

**It depends on nothing.** The lock files list this workspace and not one
package besides: no crates, no Go modules. The UEFI bindings, the disk image
writers, the settings parser, and later the cryptography and TLS, are all
written here. Every outside file Meridian will ever need (the fonts, GPU
firmware, the signed Secure Boot shim) gets its own decision record before
it lands; the fonts have theirs. Why each choice was made is in [docs/adr](docs/adr/README.md).

---

## The trick

Other projects that run Windows programs translate them. Wine maps every
Windows call onto Linux and POSIX, which works, but two worlds with
different ideas of files, processes and waiting have to be kept in step on
every call.

Meridian does not have two worlds. Its kernel is built around handles with
access rights, the same model Windows programs already expect: processes,
threads, sections, events, mutexes and files are all kernel objects reached
through handles. So the Windows layer is thin, and a Windows program and a
native Meridian app are both first class:

```
   Windows program (.exe)                 Meridian app (.mexec)
            |                                       |
   kernel32, user32, gdi32 ...                Meridian runtime
            |                                       |
     ntdll (Meridian's own)                         |
            |                                       |
            +--------------- syscall ---------------+
                                |
          +---------------------+----------------------+
          |   Meridian kernel: one object model        |
          |   processes  threads  sections  events     |
          |   mutexes  files  ... all behind handles   |
          +--------------------------------------------+
```

How Windows behaves is learned only from Microsoft's public documentation
and from black-box tests that run small programs on real Windows. Nobody
working on Meridian reads Windows source code (leaked copies included),
Wine or ReactOS ([ADR 0004](docs/adr/0004-clean-room.md)).

---

## The parts

| Part | What it is | State today |
| --- | --- | --- |
| [`boot`](docs/architecture/boot.md) | UEFI bootloader in Rust: a boot menu that finds Windows and Linux, Secure Boot through a signed shim, a framebuffer handed to the kernel without a flicker | boot screen with cards for the systems on the EFI partition and the firmware's boot options, the log, the firmware settings and `boot.meri`; loads the kernel and hands it memory, framebuffer and ACPI; the shim is not in yet |
| [`kernel`](docs/architecture/kernel.md) | hybrid kernel with the handle model above, all CPU cores from the start, NX, SMEP, SMAP and KASLR | its own descriptor tables, every exception on a panic screen with a QR code, a status screen with cores and a live uptime; a frame allocator and a heap, the APIC timer, so the processor sleeps between ticks, and every core started (M2 has begun) |
| `lib/uefi` | UEFI types and firmware calls, written from the specification | done for what the loader calls, checked against OVMF |
| `lib` | `no_std` crates shared by kernel and userland, tested on the host | `acpi`, `bootinfo`, `canvas`, `elf`, `font`, `meri`, `paging`, `qr`, `uefi`, `x86` |
| `user` | init, the shell, command-line tools, the terminal, the apps | planned (M5, M7) |
| [`win32`](docs/architecture/win32.md) | PE loader, Meridian's own ntdll, kernel32, user32, gdi32; 64-bit and 32-bit | planned (M6, M8) |
| [Horizon](docs/architecture/horizon.md) | the desktop | designed, not built (M7) |
| [Strata](docs/architecture/strata.md) | copy-on-write file system with snapshots, checksums and an instant file index | specified, not built (M4) |
| [`sextant`](docs/architecture/sextant.md) | the build and dev tool, in Go, runs on Linux, macOS and Windows | `build`, `image`, `run`, `test`, `stats`, `lint` and `mutate` work, with the `.meri` reader, the GPT, FAT32 and ISO writers and the Authenticode signer; `doctor` is next |

---

## Quick look

You need Go 1.27 or newer and rustup. rustup picks up the pinned Rust
1.99.0 from `rust-toolchain.toml` by itself. To boot Meridian you also need
QEMU and the OVMF firmware.

```
git clone https://github.com/Elchi-dev/meridian
cd meridian/tools/sextant
go build -o sextant ./cmd/sextant

./sextant run        # builds loader and kernel, writes a signed image, boots it
./sextant test       # host tests, then the QEMU test image on real firmware
./sextant ci         # every check CI runs, in its order
./sextant stats      # code and test lines per component
./sextant lint       # the project rules no compiler knows
./sextant doctor     # checks the tools you need and installs what is missing
```

The first image sextant writes on a machine makes a development key for
signing the loader, kept in your settings folder. `sign.key` in
`sextant.local.meri` names another one.

---

## Planned

What Meridian is meant to do, roughly in the order it gets built. Only the
boot menu has started to exist.

- **Windows programs.** Console tools first, then PuTTY and 7-Zip, then the
  official `python.exe` without changes. Microsoft Word is the long-term
  goal. 64-bit and 32-bit programs from the start.
- **Honest about gaps.** When a Windows program calls a function Meridian
  does not have yet, it stops with a plain message that names the DLL and
  the function, with details for those who want them. `exe-check
  program.exe` lists in advance how much of a program is covered.
- **Who lock.** Never a bare "file is in use". Meridian looks into the
  handle table and shows the exact app that holds the file or the USB drive,
  with "Close app" and "Release and delete".
- **Time slider and installer undo.** Every folder in Files has a slider
  over its Strata snapshots. Every Windows installer runs inside a snapshot,
  so a messy install can be undone completely with one click.
- **Input that never freezes.** The compositor and the cursor have CPU time
  and memory reserved, so even a fork bomb at 100 % CPU does not make the
  mouse stutter.
- **A boot menu you like.** It finds Windows and Linux by itself, shows them
  as cards and works with Secure Boot switched on, without a trip into the
  firmware settings.
- **Network drivers built in.** Common Ethernet and Wi-Fi chips work right
  after install, so getting the rest is never a problem.
- **Experiments.** Features that are not finished yet can be tried by
  switching them on in Settings, for example a global menu bar for Windows
  programs.
- **One settings format.** Every system file is `.meri`, readable by people
  and quick for programs ([spec](docs/formats/meri.md)).
- **Apps and packages.** Desktop apps are self-contained bundles. Command
  line tools come from `merinstall`, whose repository format is open, so any
  package manager can use it.

---

## Non-goals

- **Anti-cheat and other programs that install Windows kernel drivers.**
  They cannot work. Valorant will not run, and I am sad about it too :(
- **Microsoft's own system DLLs.** Every system DLL is Meridian's own.
  Loading DLLs from a Windows install may come later as an option, never by
  default, and Meridian never ships them.
- **A copy of macOS.** No global menu bar by default, no glass, no blur.
  The layout is closer to Windows; the look is Meridian's own.
- **Legacy BIOS.** UEFI only.
- **Other architectures, for now.** x86_64 first. The code keeps
  architecture specifics apart so aarch64 can follow.
- **A system only developers enjoy.** You can do anything, but you never
  have to open a terminal to get through the day.
- **Telemetry.** None, ever.

---

## Status

**M0, the foundation, and M1, the boot, are in progress.** See
[ROADMAP](docs/ROADMAP.md).

The test bar holds for every component, from the first commit on:

- At least **1.5 lines of test code for every line of code**, counted
  without comments and blank lines. `sextant stats --check` fails the build
  otherwise.
- **Mutation testing.** `sextant mutate` changes the code one mutant at a
  time, and every change no test notices has to get a test, or an entry in
  `mutants.meri` that explains why it cannot change behaviour. CI mutates
  what every push changes, with the checks on real firmware.
- **No mocks, no stubs.** Firmware code is tested against real firmware in
  QEMU, with pictures of every screen. The disk images are read back by
  `fsck.fat`, `sfdisk` and `xorriso`, QR codes by `zbarimg`, and signatures
  by OpenSSL.
  The `.meri` readers have to pass a shared conformance suite of 134 cases.

On 15 October 2026:

| Component | Code | Tests | Ratio |
| --- | --- | --- | --- |
| `boot` | 1882 | 3220 | 1.71 |
| `kernel` | 1468 | 2372 | 1.61 |
| `lib/acpi` | 147 | 369 | 2.51 |
| `lib/bootinfo` | 167 | 262 | 1.56 |
| `lib/canvas` | 292 | 506 | 1.73 |
| `lib/elf` | 265 | 523 | 1.97 |
| `lib/font` | 898 | 2616 | 2.91 |
| `lib/frames` | 159 | 256 | 1.61 |
| `lib/heap` | 49 | 120 | 2.44 |
| `lib/meri` | 1240 | 1962 | 1.58 |
| `lib/paging` | 182 | 509 | 2.79 |
| `lib/qr` | 496 | 830 | 1.67 |
| `lib/uefi` | 1666 | 2666 | 1.60 |
| `lib/x86` | 634 | 955 | 1.50 |
| `tools/sextant` | 7712 | 13743 | 1.78 |
| total | 17257 | 30909 | 1.79 |

`sextant stats` prints the current numbers.

---

## Docs

| Document | What |
| --- | --- |
| [ROADMAP](docs/ROADMAP.md) | milestones, what is in and out, and why |
| [TODO](TODO.md) | open work, the only issue tracker |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | how the parts fit, and one page per big part |
| [testing](docs/testing.md) | host tests, QEMU test images, mutation testing |
| [.meri format](docs/formats/meri.md) | the settings and system file format |
| [decision records](docs/adr/README.md) | every decision, with the alternatives that lost |
| [CONTRIBUTING](CONTRIBUTING.md) | building, testing and the rules |
| [SECURITY](SECURITY.md) | reporting a vulnerability |
| [CHANGELOG](CHANGELOG.md) | what changed |

---

## Licence

MIT or Apache-2.0, at your choice ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)).

Made by Elchi
