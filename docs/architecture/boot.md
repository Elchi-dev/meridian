# The loader

`boot/` builds `meridian.efi`, a UEFI application written in Rust for the
`x86_64-unknown-uefi` target. UEFI only: no legacy BIOS boot.

## What exists today

- **Entry point.** `efi_main` takes the system table from the firmware and
  checks it before anything else: the signature and size of the system table
  and of the boot services table. A table that fails the check stops the
  loader with `EFI_LOAD_ERROR`.
- **Serial log.** COM1 at 115200 baud, 8 data bits, no parity, 1 stop bit,
  FIFOs on. The driver is in `lib/x86`, shared with the kernel: the
  register values in `serial.rs`, tested on the host, and the port writes
  in `raw.rs`. A board without COM1 reads 0xff from the line status
  register, so writes go nowhere and nothing hangs.
- **Banner.** The loader names itself, the firmware vendor and revision,
  and the UEFI version. Text for people goes to the firmware console only,
  and to the serial port only when there is no console. OVMF already copies
  its console to the serial port, so writing to both would print every line
  twice.
- **Loading the kernel.** It reads `\meridian\kernel` from the partition
  it was started from, checks the ELF file (`lib/elf`), makes sure the entry
  point lies in code, and loads the segments into kernel memory, applying
  relocations.
- **The kernel's address space** (`handover.rs`, built with `lib/paging`):
  - each kernel segment with the rights it asks for, never writable and
    executable at once
  - all physical memory, and at least the first 4 GiB, at `0xffff800000000000`,
    writable and never executable, in 1 GiB pages where the processor has them
  - the stack below `STACK_TOP` with an unmapped guard page
  - the loader's own image at its own address, so the instructions right
    after the switch are still there; nothing else of the lower half, not
    even address 0
- **The handover.** Everything the kernel keeps (its image, the stack, the
  page tables, the boot information) is allocated with the memory type
  `0x80000000`, so it shows up as kernel memory in the map. The loader reads
  the final memory map into the boot information, leaves boot services
  (trying a second time if the map changed in between), turns on no-execute
  pages and write protection, switches page tables and stack and calls the
  kernel. If anything fails before that, it says what in a sentence, waits
  for a key and returns to the firmware.
- **Test image.** Built with the `qemu-test` feature, the loader runs the
  QEMU checks of `lib/uefi`, `lib/x86` and its own against OVMF, then
  starts the kernel's test build, which checks the handover. Every check is
  reported over the serial port, and the run ends with QEMU exit code 33
  (all passed) or 35 (something failed). A failed loader check ends the run
  before the kernel starts. Port 0xf4 is only written in test images. See
  [testing.md](../testing.md).

The loader's own `unsafe` code, the entry point and the panic handler, is
in `boot/src/raw.rs`. `lib/uefi` keeps its firmware tables in `src/raw/`,
and `lib/x86` its instructions (port I/O, CPUID, control registers,
halting, the QEMU exit) in `src/raw.rs` (ADR 0008).

## M1: what it becomes

### The boot screen

- **Look.** A graphical screen in the boot screens' fixed dark look, drawn into the GOP framebuffer with `lib/canvas` and Inter. The loader draws each frame off the screen and copies it whole, so nothing tears. Cards are solid with a 1 px border and a solid offset shadow; the chosen one gets a tangerine frame and shadow. Exists.
- **The menu.** Systems appear as cards, each with a tile in its colour and the first letter of its name. Left and Right choose, Enter starts, a countdown of 3 seconds starts the chosen one, and any key stops it. When more systems are found than cards fit, the row follows the choice. Exists.
- **Keys.** V shows the log at any time (the transparency Samuel asked for: "full power if the user is sure he wants it"): every line the loader wrote, in JetBrains Mono, the newest that fit, and any key goes back. It names the firmware, the screen, whether Secure Boot is on, whether the firmware settings can be opened, and each system found with its path. F opens the firmware settings: the loader sets the `BOOT_TO_FW_UI` bit of `OsIndications` and restarts, and the firmware starts its own menu. F is only offered, with a key hint under the cards, when the firmware's `OsIndicationsSupported` says it can. Exists; a Go test presses F on OVMF and sees its setup menu after the restart.
- **Text.** A TrueType rasterizer written for Meridian, with grayscale anti-aliasing and no hinting. It is shared with the kernel and Horizon through a `lib/` crate. The font files (Inter and JetBrains Mono) are the only outside files, under one decision record.
- **The Windows card** gets a W on a sky blue tile, not Microsoft's logo.

### Finding other systems

What exists: the loader looks through every folder of `\EFI` except
`BOOT` and `meridian` for the loaders it knows, best first: Windows Boot
Manager (`Boot\bootmgfw.efi`), systemd-boot, a shim before GRUB (so the
system keeps starting with Secure Boot on), and rEFInd. A folder named
after a known distribution gets its name ("arch" shows as Arch Linux).
Each one found gets a card behind Meridian's, and choosing it loads and
starts that loader from the same partition through `LoadImage` and
`StartImage`. If it comes back, the boot screen comes back. Test images
carry a second copy of the loader as `\EFI\example\grubx64.efi`, which
the test image finds, starts from the boot screen with Right and Enter,
and sees come back.

The firmware's own list counts too. After the scan, the loader reads
`BootOrder` and each `Boot####` and offers a card for every option that is
active, not hidden, a boot option rather than an application, and names a
file. It leaves out the option the firmware started it with
(`BootCurrent`), and options for a file that has a card already on the
partition or CD it sits on, so neither Meridian itself nor a system the
scan found shows twice. Systems on other disks, such as Windows on its
own drive, come in this way. Such an option is loaded through its whole
device path; when the loader cannot follow the path, as with the short
ones Windows writes that start at the partition, it sets `BootNext` and
restarts, and the firmware starts the option itself. The test image adds
three options before the loader looks: one it starts from the third card
and that comes back, one switched off and one for a loader the scan finds
anyway, which must get no card.

Still to come:

- **Scanning.** The EFI partition is also scanned for well-known loaders: `\EFI\Microsoft\Boot\bootmgfw.efi`, GRUB, systemd-boot.
- **Starting one.** A chosen system is started through `LoadImage` and `StartImage` on its device path. With Secure Boot on, the firmware checks it, and Windows Boot Manager passes because Microsoft signs it.

### Secure Boot

```
  firmware  --checks-->  shimx64.efi (signed by Microsoft's UEFI CA)
                              |  checks against the MOK list
                              v
                         meridian.efi (signed by sextant with Meridian's key)
```

- **The shim** is a prebuilt, Microsoft-signed binary from a Linux distribution, kept under `third-party/` with its own decision record. It goes to `\EFI\BOOT\BOOTX64.EFI`, and shim's default next stage is where `meridian.efi` sits.
- **The `.sbat` section.** `meridian.efi` carries an `.sbat` section, which current shims require: the SBAT format line and Meridian's own record, generation 1. Exists; a Go test reads it from the signed loader of a debug and a release build, since link-time optimisation would drop a section nothing uses.
- **Signing.** sextant signs `meridian.efi` with an Authenticode signer of its own, written with Go's standard library. Exists; see [sextant.md](sextant.md#signing).
- **The key.** On the very first boot shim cannot check Meridian's signature yet and opens MokManager. The person picks "Enroll key from disk" and Meridian's certificate once. Every later boot is silent, and no firmware setting has to change.
- **Release and development keys.** Release images are signed with the project key, kept as a CI secret. Development images use a key that sextant makes on each machine.
- **Tested against real firmware.** Ubuntu's OVMF comes with a Secure Boot build and a variable store that holds its "snakeoil" test key as PK, KEK and db, and with that key. A Go test signs the loader with it, and OVMF starts it with Secure Boot on, through to the kernel; the same loader unsigned, or signed with a key nobody enrolled, is refused with "Access Denied". A loader signed over a wrong digest is refused too, so the test can fail.
- **The kernel is not checked yet.** Checking its signature needs Meridian's own SHA-256 and signature code; until then the chain of trust ends at the loader.

### Settings

`\meridian\boot.meri` on the EFI partition, which every image sextant
writes carries with the defaults spelled out:

```
boot {
    timeout = 3s        # 0s waits for a key; at most 10 minutes
    default = meridian  # a card's name in any case, or last
    verbose = false     # true opens the boot screen with the log
}
```

The loader reads it with `lib/meri`, the Rust `.meri` reader, which passes
the same conformance suite as sextant's Go reader and needs no heap. A
setting that is missing or wrong keeps its default, and the log says which
line was wrong and why. With `default = last`, the loader keeps the name of
the system it starts in the firmware variable `LastSystem` of Meridian's
own vendor GUID, and writes it only when it changes, since firmware
variables live in flash. With `verbose = true` the countdown runs under the
log, and its line says which system starts when. Exists; the test image
checks it read its own `boot.meri`, and a Go test boots the plain image
with `verbose` and `default = example` and sees the example start.

### Starting the kernel

Loading, the page tables and the handover exist (see above), with
Meridian's own protocol rather than Multiboot or Limine. Still to come:

- **The framebuffer** (address, size, stride, pixel format) and the boot
  choices join the boot information.
- **No flicker.** The framebuffer is handed over as it is, with the boot screen still on it, so the kernel continues from the same picture.
