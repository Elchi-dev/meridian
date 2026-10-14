# The kernel

Nothing on this page exists yet. It describes the design decided in
QUESTIONS.md and the decision records, and the milestone in which each part
is built.

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
- **Panic screen.** It says in plain English what went wrong and what to do
  next, and shows a QR code of a short crash summary (build, event, address,
  instruction pointer, symbol, CPU, uptime). A QR code holds about 2.9 KB, so
  it is a summary, not a dump. A QEMU test triggers a panic and checks the
  screen.

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
