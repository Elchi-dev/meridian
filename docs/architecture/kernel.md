# The kernel

This page describes the design decided in QUESTIONS.md and the decision
records, and the milestone in which each part is built. The first section
is what exists today; everything after it is still to come.

## What exists today

- **The crate.** `kernel/` builds a position independent ELF file for
  `x86_64-unknown-none`, linked by `kernel/link.ld` into the top 2 GiB at
  `0xffffffff80000000`. Code, read-only data and writable data each start
  on a page of their own, so the loader can map each with exactly its
  rights. sextant puts the file at `\meridian\kernel` on the EFI partition.
- **The way in.** The loader calls `kernel_start` with the boot information
  (`lib/bootinfo`) in `rdi`, on a 64 KiB stack below `STACK_TOP` with an
  unmapped guard page under it, paging on, no-execute pages and write
  protection on, and interrupts off. The kernel refuses boot information
  whose magic, version or size it does not know.
- **What it does.** It opens COM1 through `lib/x86`, prints its version and
  how much memory is free and how much it owns, draws the status screen
  and halts.
- **The status screen.** In the boot screens' dark look, drawn with
  `lib/canvas` and Inter: the kernel's version, the processor's name from
  CPUID, the free memory out of all memory, and the memory map as a bar.
  Reserved ranges above the end of memory are devices, such as QEMU's
  12 GiB PCI window, and are left out.
- **The panic screen.** A panic draws a sun yellow screen that says in
  plain words what happened and what to do, with the panic message and
  where it came from, and a QR code of a short summary (version, event,
  message, place) at error correction level M, made by `lib/qr`. The
  handler takes the framebuffer even if it was handed out before, keeps its
  working space out of the stack, and draws only once: a panic while
  drawing stops at once.
- **Test image.** Built with the `qemu-test` feature, it checks what the
  loader promised: the direct map, its own image mapped at the right
  physical memory, the stack, the processor state, the ACPI root pointer,
  and that boot services are over. It checks its status screen by reading
  the framebuffer back, then panics on purpose, checks the panic screen
  the same way and ends the QEMU run with `TESTS DONE`. sextant keeps both
  screens as PNG files, and a Go test reads the QR code off the panic
  screen with `zbarimg`.

## Shape

- **Hybrid** (QUESTIONS.md 7.1): memory, scheduling, the object manager, IPC, interrupts and drivers in the kernel; the compositor, the service manager and other services as user processes.
- **x86_64 only for now.** Architecture code lives in `kernel/src/arch/x86_64/`, so aarch64 can be added without touching the rest.
- **x86-64-v2 as the minimum** (SSE4.2, POPCNT, about what Windows 11 requires). Faster paths for AVX2 are chosen at run time.
- **All CPU cores from M2 on.** Adding several cores later would mean rewriting the locking.
- **Built with stable Rust** for `x86_64-unknown-none` (ADR 0006). Assembly is written inside Rust (ADR 0007), and every `unsafe` is in a `raw` module (ADR 0008).

## Memory (M2)

- a higher-half kernel, with a direct map of all physical memory
- a physical frame allocator built from the loader's memory map
- page tables with NX on everything that is not code, W^X everywhere, and guard pages below every stack
- SMEP and SMAP on from the start; KASLR from M3

## Objects and handles (M3)

Every resource is a kernel object: process, thread, section, event, mutex,
semaphore, timer, file, IPC channel. A process reaches objects only through
handles, each with an access mask, in its own handle table. Waiting works
the same for every waitable object, which is exactly what `WaitForSingleObject`
and `WaitForMultipleObjects` need. Every process carries a security token.

The handle table is also what makes "who lock" possible: the System monitor
and Files can ask which process holds a handle to a given file.

## Scheduling (M3)

Desktop responsiveness first: the window under the mouse never stutters
because something compiles in the background. 32 priority levels, so
Windows thread priorities map one to one. The compositor and the cursor get
reserved CPU time and memory, so input stays smooth even with every core at
100 %.

## IPC (M3)

Typed message channels that can carry handles, used by every service and by
the Windows personality.

## The screens the kernel draws itself (M1)

- **Status screen.** The CPU cores found in the ACPI MADT, the memory map
  as a bar, and the uptime, updated live.
- **Panic screen.** It exists (see above). Still to come in the summary:
  the build's commit, the instruction pointer and symbol of a CPU
  exception, the CPU and the uptime. A QR code holds about 2.9 KB, so it
  stays a summary, not a dump.

## Drivers

Rust drivers in the kernel, in this order:

| When | Drivers |
| --- | --- |
| M4 | virtio-blk, AHCI, NVMe |
| M7 | PS/2, xHCI with USB HID |
| later | virtio-net and common Ethernet and Wi-Fi chips, HD Audio, ACPI with AML, GPUs |

Driver code from elsewhere may only be translated into Rust from MIT, BSD
or ISC licensed sources, never GPL (ADR 0002, 0003). Vendor firmware files
are allowed as data, each with its own record.
