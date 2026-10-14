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
- **Then it waits.** There is no kernel yet, so the loader says so, waits
  for a key and returns to the firmware.
- **Test image.** Built with the `qemu-test` feature, the loader runs the
  QEMU checks of `lib/uefi`, `lib/x86` and its own against OVMF, reports each over the
  serial port and ends QEMU with exit code 33 (all passed) or 35 (something
  failed). Port 0xf4 is only written in test images. See
  [testing.md](../testing.md).

The loader's own `unsafe` code, the entry point and the panic handler, is
in `boot/src/raw.rs`. `lib/uefi` keeps its firmware tables in `src/raw/`,
and `lib/x86` its instructions (port I/O, CPUID, control registers,
halting, the QEMU exit) in `src/raw.rs` (ADR 0008).

## M1: what it becomes

### The boot screen

- **Look.** A graphical screen in Horizon's look, drawn into the GOP framebuffer. The loader keeps an off-screen copy and copies whole frames, so nothing tears.
- **The menu.** Systems appear as cards: Meridian, plus every other system found. Left and Right choose, Enter starts, a countdown starts the last choice.
- **Keys.** V shows the full log at any time (the transparency Samuel asked for: "full power if the user is sure he wants it"). F opens the firmware settings.
- **Text.** A TrueType rasterizer written for Meridian, with grayscale anti-aliasing and no hinting. It is shared with the kernel and Horizon through a `lib/` crate. The font files (Inter and JetBrains Mono) are the only outside files, under one decision record.
- **The Windows card** gets a neutral icon, not Microsoft's logo.

### Finding other systems

- **The firmware's entries.** The firmware variables `BootOrder` and `Boot####` give the other systems the firmware knows about, with their descriptions and device paths.
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
- **The `.sbat` section.** `meridian.efi` carries an `.sbat` section, which current shims require.
- **Signing.** sextant signs `meridian.efi` with an Authenticode signer of its own, written with Go's standard library.
- **The key.** On the very first boot shim cannot check Meridian's signature yet and opens MokManager. The person picks "Enroll key from disk" and Meridian's certificate once. Every later boot is silent, and no firmware setting has to change.
- **Release and development keys.** Release images are signed with the project key, kept as a CI secret. Development images use a key that sextant makes on each machine.
- **The kernel is not checked yet.** Checking its signature needs Meridian's own SHA-256 and signature code; until then the chain of trust ends at the loader.

### Settings

`\meridian\boot.meri` on the EFI partition: timeout, default entry (or
"last"), verbose. It is read by the Rust `.meri` reader, which passes the same
conformance suite as sextant's Go reader.

### Starting the kernel

- **Its own handover.** Meridian's own boot protocol, not Multiboot or Limine.
- **Loading.** The loader reads `\meridian\kernel` (ELF64), checks every header field and loads the segments.
- **Page tables.** It builds the tables: the kernel in the higher half, a direct map of physical memory, and the loader's identity map until the jump.
- **The handover structure.** The kernel gets its own memory map format, the framebuffer (address, size, stride, pixel format), the ACPI RSDP, the kernel's physical base and the boot choices.
- **No flicker.** The framebuffer is handed over as it is, with the boot screen still on it, so the kernel continues from the same picture.
