# Roadmap

Where Meridian goes, in what order, and why that order. Each milestone
says what is in, what is deliberately out, and when it counts as done.
[TODO.md](../TODO.md) tracks the single items.

The order follows one rule: build what everything else stands on first,
and never build something that cannot be tested yet.

| | Milestone | State |
| --- | --- | --- |
| M0 | Foundation | in progress |
| M1 | Boot | in progress |
| M2 | Memory and CPUs | done |
| M3 | Processes | in progress |
| M4 | Storage and Strata | in progress |
| M5 | Userland | in progress |
| M6 | Windows programs, step 1: the console | planned |
| M7 | Horizon, the desktop | planned |
| M8 | Windows programs, step 2: windows | planned |
| Later | Networking, power, GPU, sound and more | far away |

## M0: Foundation

**In:**

- The repository: licences, the decision records, CI, the docs.
- sextant, the build tool, on Linux, macOS and Windows:
  - `stats` and `lint`, so the 1.5 test ratio and the project rules hold from the first commit
  - `mutate`, so tests are proven able to fail
  - `doctor`, which checks the tools and installs them after one "y"
  - `build`, `image`, `run` and `test`
- The `.meri` format with its Go reader and a conformance suite.
- GPT, FAT32 and ISO writers that need no outside tools and give the same bytes on every machine.
- `lib/uefi`, the UEFI types and calls Meridian uses.
- A loader that boots in QEMU and checks the firmware.

**Out:** the kernel, and everything a person would call an operating system.

**Why first:** every later milestone needs a way to build an image, boot it
and see whether its tests passed. The rules (ratio, mutation testing, no
dashes, no outside code) are cheap to hold from the start and expensive to
introduce later.

**Done when:** `sextant ci` runs the same checks locally and in GitHub
Actions, all green, and `sextant run` boots the loader from an image that
sextant wrote.

## M1: Boot

The first thing anyone sees of Meridian, and the milestone shown at the
first check-in.

**In:**

- **Look.** A graphical boot screen in Meridian's own look: solid, colourful, no text console. It is drawn by a TrueType rasterizer written for Meridian, with Inter and JetBrains Mono as the only outside files, under one decision record.
- **Finding other systems.** The loader reads the firmware's boot entries and scans the EFI partition for Windows Boot Manager, GRUB and other loaders, and shows each system as a card. Arrow keys and Enter choose, a countdown starts the last choice, and the full log is one key away.
- **Starting other systems.** Windows and Linux are started from their cards.
- **Secure Boot without firmware settings.** The first stage is a shim signed by Microsoft's UEFI CA (an outside binary with its own record), which then starts `meridian.efi`. sextant signs `meridian.efi` with Meridian's key, using an Authenticode signer of its own. On the first boot the person enrolls that key once in the shim's MokManager screen ("Enroll key from disk"); after that, every boot is silent. A shim of Meridian's own, reviewed and signed by Microsoft, would remove that step and is a long-term goal.
- **Settings.** `boot.meri` on the EFI partition, read by the Rust `.meri` reader, which has to pass the same conformance suite as the Go reader.
- **Starting the kernel.** The loader loads the kernel's ELF file, builds the page tables for a higher-half kernel and hands over the memory map and the framebuffer without clearing the screen, so there is no flicker.
- **Kernel basics.**
  - serial log and framebuffer text
  - GDT and IDT, and every CPU exception caught
  - a live status screen with the CPU cores found in the ACPI MADT, the memory map and the uptime
- **Panic screen.** It explains in plain English what happened and shows a QR code of a short crash summary. A QEMU test checks it.
- **Real hardware.** M1 is checked on a laptop that dual boots Windows and Arch Linux with Secure Boot switched on.

**Out:**

- starting the other CPU cores (they are found, not started)
- memory management beyond the boot map
- user mode

**Not covered by Secure Boot yet:** the kernel itself. Firmware and shim
check the loader, but checking the kernel's signature needs Meridian's own
SHA-256 and signature code. Until that exists, the chain of trust ends at
the loader. This is written into [SECURITY.md](../SECURITY.md) as well.

**Why now:** the boot path is needed for every test on real hardware, and
Secure Boot decides whether Meridian starts on a normal laptop at all.

## M2: Memory and CPUs

**In:**

- a physical frame allocator and virtual memory, and the kernel heap
- local APIC and x2APIC, HPET and TSC timers
- every CPU core started, which means 12 hardware threads on the development PC
- the ACPI tables parsed, without AML yet

**Why here:** every later part needs memory, and supporting several cores
from the start avoids rewriting all the locking later.

## M3: Processes

**In:**

- **The scheduler.** Desktop responsiveness comes first. Its 32 priority levels map one to one to Windows priorities.
- **The kernel boundary.** User mode, system calls, and the handle-based object model with access rights. Every process carries a security token from the start, because Windows programs expect one.
- **Native programs.** Native programs are `.mexec` files: ELF64 with Meridian's metadata (icon, permissions, sandbox) in ELF note sections, so `readelf` and `gdb` keep working. A loader for them.
- **The rest.** IPC channels that can carry handles, and KASLR.

**Out:** multi-user in the UI (the model supports it, the UI starts with one
user), app sandboxing (optional, later; camera and microphone will always
ask).

## M4: Storage and Strata

**In:**

- **Drivers.** virtio-blk, AHCI and NVMe.
- **Partitions and FAT.** GPT, and FAT32 for the EFI partition.
- **Strata version 1:**
  - copy-on-write
  - checksums on data and metadata
  - snapshots
  - names that ignore case but keep it, as Windows programs expect
  - extended attributes and alternate data streams
  - the instant locate index: a B-tree and trigram index updated on every write, for sub-millisecond file search without an indexing daemon
- **Room for later.** The on-disk format reserves room from day one for multi-file transactions, content-addressed deduplication with instant clones, and files that expire by themselves, so the format never has to break.
- **The VFS.**
- **Disk safety on real hardware.** Meridian never writes to a partition it did not create, unless the person unlocks it in Settings.

**Out:**

- transparent compression and encryption (later; encryption needs Meridian's own cryptography)
- USB mass storage, exFAT, NTFS, ext4

## M5: Userland

**In:**

- **System startup.** init and the service manager.
- **The shell.** Meridian's own shell with its own language, and an `sh` mode switched on through an environment variable, so existing scripts run. `cls` and `clear` both work.
- **Command-line tools.** Linux names, plus friendlier names where those help people who rarely use a terminal.
- **The terminal.** A text terminal, compatible with xterm, with true colour and full Unicode.

## M6: Windows programs, step 1: the console

**In:**

- **Loading.** A PE loader for 64-bit and 32-bit programs from the start. 32-bit programs run in the CPU's compatibility mode, and every Meridian DLL is built twice.
- **The DLLs.** Meridian's own ntdll and kernel32, and the C runtime parts that console programs use.
- **What Windows looks like to a program.** Meridian reports itself as Windows 10 22H2 (10.0.19045). The registry is a real store on disk. `C:\` is `/Windows`, and `C:\Users\<name>` is the person's real home folder.
- **Missing functions.** A program starts even when it imports functions Meridian lacks. It stops with a clear message only when it actually calls one. `exe-check` reports coverage in advance.
- **Testing against real Windows.** Small test programs are built with `clang-cl` and `lld-link`. Their output is recorded on GitHub's Windows runners and has to match on Meridian.

**Done when:** command-line tools built with MSVC or clang-cl against the
Universal CRT run and match their Windows output.

## M7: Horizon, the desktop

**In:**

- **Drawing.** A compositor that draws on the CPU into the UEFI framebuffer, with SIMD and damage tracking. virtio-gpu in QEMU.
- **Windows and text.** Window management, TrueType text, and vector icons drawn by a renderer for a small SVG subset.
- **Input.** PS/2 keyboard and mouse, and USB HID over xHCI. Without USB HID, most real machines have no input.
- **The desktop.** A taskbar at the bottom, a launcher that searches apps, files and settings, a window overview and workspaces, the notification centre, the control centre and widgets.
- **The apps.** Terminal, Files with the time slider, Settings (every part of the look, and the Experiments page), the System monitor (every process, handle and driver), the Image viewer, and Clock and calendar.
- **The UI toolkit.** A declarative toolkit in Rust.
- **Keyboard.** Ctrl shortcuts as on Windows by default; Super as Cmd is a setting. Layouts come from Unicode's CLDR data, which gets its own decision record.
- **Input that never freezes.** CPU time and memory are reserved for the compositor and the cursor.

**The look:** opaque surfaces, crisp 1 px borders, no gradients, no blur, a
joyful multi-colour palette, windows-style layout with window buttons on the
right. Its tokens come from the prototype made before any compositor code.

**One screen only, for now.** UEFI hands over a single framebuffer, so
Meridian on real hardware uses one monitor until it has a GPU driver.
Several monitors with different resolutions and refresh rates (such as
1920x1080 at 60 Hz next to 1920x1200 at 59.95 Hz) are built on virtio-gpu
in QEMU first, which has several heads.

## M8: Windows programs, step 2: windows

**In:**

- **Windows and drawing.** user32 and gdi32, so GUI programs open real windows on Horizon.
- **Controls.** Standard controls are drawn in Meridian's look. Menus stay inside the window; the global menu bar is an experiment.
- **Who lock.** It shows which app holds a file, with "Close app" and "Release and delete".
- **Installer undo.** Every installer runs inside a Strata snapshot, so one click undoes it.
- **First programs.** PuTTY and 7-Zip.

## Later, in this order

From Samuel's priority list, after the desktop basics:

1. **USB beyond keyboard and mouse.** Mass storage and hubs.
2. **Networking.** virtio-net, Realtek RTL8168 and other common chips. A TCP/IP stack with IPv4 and IPv6 together. TLS 1.3 and its cryptography written here and tested against the official vectors.
3. **Wi-Fi.** Common chips with their vendor firmware, each firmware file with its own record, and WPA2 and WPA3 on Meridian's own cryptography.
4. **Power.** An ACPI AML interpreter for shutdown, sleep, the laptop battery and interrupt routing on real hardware. The laptop touchpad (I2C HID) depends on it and follows.
5. **GPU acceleration.** AMD GPUs need AMD's signed firmware files. Several monitors on real hardware come with it, and Direct3D 11 on top of Meridian's GPU interface follows.
6. **Sound.** HD Audio.
7. **Bluetooth.**

Also later, without a fixed place:

- **Windows programs.**
  - the official `python.exe` without changes
  - .NET programs, through Microsoft's own open-source .NET runtime running as a Win32 program
  - loading DLLs from a Windows install, as an option
- **File systems and moving in.** NTFS read (with an import from Windows for people switching), exFAT, ext4 read. An installer to a real disk.
- **Other systems' programs.** A POSIX layer for porting C programs, then Linux binary compatibility in the style of WSL 1.
- **Packages.** `merinstall` and a central package repository, with third-party repositories like apt's.
- **Platform.** A shim of Meridian's own through Microsoft's review, and aarch64.

## The North Star

Microsoft Word, the current version, running unchanged. It is not part of
any milestone above, and it is years away. Word needs:

- Office's installer and licensing service
- a sign-in window built on Edge WebView2, which means Chromium running on Meridian
- DirectWrite and Direct2D
- a very large share of the Win32 API

Every step above moves toward it. None of them gets close on its own.

## Targets that will be measured

| | QEMU with KVM | Development PC |
| --- | --- | --- |
| Firmware handoff to desktop | under 1 s | under 3 s |
| Desktop idle memory | under 300 MiB | |
| Keypress to pixel in the terminal | under 10 ms | |

None of these can be measured before M7.
