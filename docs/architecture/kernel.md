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
  how much memory is free and how much it owns, reads the ACPI tables,
  measures the time stamp counter, draws the status screen and then keeps
  the uptime on it current.
- **ACPI and time.** `lib/acpi` follows the root pointer the loader passed
  to the XSDT, counts the processor cores in the MADT that are enabled or
  can be brought online, finds the power management timer in the FADT,
  with its port and whether it counts in 24 or 32 bits, and the HPET in its
  own table. The kernel maps the HPET's registers uncached at `HPET_PAGE`,
  next to the APIC's, and lets its main counter run. The kernel reads
  tables only where the memory map has a region and below the end of the
  direct map, so a broken pointer cannot fault. It measures the time stamp
  counter against the ACPI timer for 10 ms, or against the HPET where the
  ACPI timer is missing or does not move, and gives up after two million
  reads of a clock that does not move. The log says which clock it used.
  The test image checks the HPET is found, mapped uncached and counting,
  and that the counter measured against it agrees with the ACPI timer to
  within 2%. The uptime is the counter divided by
  its rate: the time since the machine was switched on. The local APIC runs in x2APIC mode, through model specific registers
  rather than memory, where the processor has one; otherwise in xAPIC
  mode, through its page of registers, which the kernel maps uncached at
  `APIC_PAGE` in a slot of the root table of its own. In xAPIC mode an
  interrupt command goes in two halves, after the last one was delivered.
  Its timer is measured against the time stamp counter
  for a hundredth of a second and then interrupts a hundred times a
  second, the legacy 8259s are masked, and the processor sleeps with `hlt`
  between ticks, drawing the uptime again when the second changes. Without
  a local APIC or a known counter rate it spins as before. `sextant test
  --qemu` boots the test image twice, the second time on a processor
  without an x2APIC, so both modes are checked on every run. The test image
  checks the APIC was off before, sees about ten ticks in a tenth of a
  second, a halted processor wake at the next one, and the timer stop.
- **The status screen.** In the boot screens' dark look, drawn with
  `lib/canvas` and Inter: the kernel's version, the processor's name from
  CPUID, the number of cores, how long the machine has been up, the free
  memory out of all memory, and the memory map as a bar.
  Reserved ranges above the end of memory are devices, such as QEMU's
  12 GiB PCI window, and are left out. Each frame, and each new uptime, is
  drawn off the screen in the room the loader set aside and then copied
  over, so the person never sees a frame half drawn.
- **Descriptor tables and exceptions.** At entry the kernel loads its own
  GDT (null, code, data, task state), a task state segment and an IDT
  with a gate for each of the 32 exceptions. Each gate leads to a small
  stub that pushes the vector, and a 0 where the processor gives no error
  code, so `exception` always finds the same frame. Double faults, NMIs and
  machine checks switch to a 16 KiB fault stack, so running out of stack
  still ends on the panic screen instead of resetting the machine. An
  exception is shown in words, such as "page fault: the kernel read 0x0,
  which is not mapped", with the address of the instruction.
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
  and that boot services are over. It checks the cores and the timer it
  found, and measures the counter a second time over 100 ms, which has to
  agree with the first measurement to within a tenth. It checks its status screen by reading
  the framebuffer back, and checks the loaded tables and five exceptions
  it causes on purpose (breakpoint, invalid opcode, division by zero, a
  read of address 0, a bad selector), each resumed after the fault. Then
  it panics on purpose and checks the panic screen, and then runs out of
  stack on purpose and checks that the double fault reaches the panic
  screen too. sextant keeps the three screens as PNG files, and a Go test
  reads both QR codes with `zbarimg`. Another Go test boots the plain
  image, keeps its status screen twice, 2.5 seconds apart, and checks that
  only the uptime changed.

## Shape

- **Hybrid** (QUESTIONS.md 7.1): memory, scheduling, the object manager, IPC, interrupts and drivers in the kernel; the compositor, the service manager and other services as user processes.
- **x86_64 only for now.** Architecture code lives in `kernel/src/arch/x86_64/`, so aarch64 can be added without touching the rest.
- **x86-64-v2 as the minimum** (SSE4.2, POPCNT, about what Windows 11 requires). Faster paths for AVX2 are chosen at run time.
- **All CPU cores from M2 on.** Adding several cores later would mean rewriting the locking.
- **Built with stable Rust** for `x86_64-unknown-none` (ADR 0006). Assembly is written inside Rust (ADR 0007), and every `unsafe` is in a `raw` module (ADR 0008).

## Cores (M2)

Every core starts. The first one reads the MADT for the others' APIC
ids, copies a trampoline to the first free memory below 640 KiB with
page tables of its own after it, and sends each core INIT and two
startup interrupts through the local APIC. The trampoline runs from real
mode through protected mode into long mode on its tables, which map the
trampoline where it is and the kernel's half as the kernel has it, and
jumps into the kernel, which switches the core to the kernel's page
tables and a 16 KiB stack of its own. There it loads the kernel's GDT
and IDT and a task state of its own, which the first core set up before
it started it, with a 16 KiB fault stack of its own: NMIs, double faults
and machine checks on it run there, as they do on the first core. Then
it turns its APIC on and halts, as there is no work for it yet. The GDT
has room for the task states of 64 cores; cores past them are not
started. The test image checks every core came in on its own
stack and the kernel's tables, and that starting them did not wait out
the time a core that never starts gets.

## Memory (M2)

- a higher-half kernel, with a direct map of all physical memory
- a physical frame allocator built from the loader's memory map. Exists: `lib/frames` keeps one bit for each 4 KiB frame, set while it is free, in free memory it then takes out for itself, and hands out nothing below the first MiB, which stays for starting the other cores. Single frames come from the lowest free one on; runs of frames come on any power of two alignment
- the kernel heap. Exists: 4 MiB of frames on a 2 MiB boundary, in blocks of 16 bytes kept in the same kind of bitmap (`lib/heap`), behind Rust's global allocator, so `Box`, `Vec` and `String` work in the kernel. Both sit behind spin locks, ready for more cores
- the test image hands out frames and checks they are free memory above 1 MiB, each once, and keep what is written to them; takes a 2 MiB run; gives all back; uses the heap and sees every byte come back
- page tables with NX on everything that is not code, W^X everywhere, and guard pages below every stack
- SMEP and SMAP on every core where the processor has them, so the kernel neither runs a program's code nor touches its pages by mistake; the test image calls into a page of ring 3 and reads one, and checks both fault
- KASLR: the loader starts the kernel at one of about a thousand 2 MiB boundaries in the top 2 GiB, picked by RDRAND

## Ring 3 (M3)

What exists: programs run in ring 3. The GDT has code and data segments
for ring 3, user data right before user code as `sysret` will want it.
Every program gets an address space of its own: a level 4 table whose
kernel half is a copy of the kernel's, and whose lower half holds the
program's image from 4 MiB on and 64 KiB of stack below 8 MiB, with an
unmapped page under it. Every slot of the kernel's half has a table from
boot on, so whatever the kernel maps later shows in every address space.
A thread of its own enters the program with `iretq`, its argument in rdi
and every other general register cleared, and every switch loads the
next thread's address space. On every switch the task state gets the
next thread's kernel stack, so an interrupt or system call from ring 3
lands on the right one. Coming in from ring 3 the kernel puts its own
stack segment back, since the processor leaves a null one that the next
thread on the core would keep, and clears the AC flag, which a program
can set to switch SMAP off for the kernel.

System calls go through `int 0x2e`, a gate ring 3 may use, a nod to
Windows NT's first system call gate: the number in rax, arguments in rdi,
rsi, rdx and r10, the answer back in rax, errors counting down from the
top of the range. `lib/abi` holds the numbers, the errors and how a call
is written into registers, for the kernel and for programs alike. There
are thirteen: exit, log a line, give way, sleep, ask for the ticks, the
four of channels (see IPC), map and unmap memory, and spawn a program and
wait for it. The kernel only
reads and writes what lies in the lower half and is mapped for ring 3,
and only inside `with_user_access`, the one place it lets itself past
SMAP. A fault in ring 3 ends only the program; the kernel writes the
exception, its error code and where it happened to its log, and goes on.
When a program ends, its handles are closed and its whole address space
goes back to the frame allocator, every table and page of its lower half
once.

**Programs from files.** The loader reads every file in
`\meridian\programs` into kernel memory and lists them in the boot
information, and the kernel starts each one that is a `.mexec` file: an
ELF file whose Meridian note says so ([the format](../formats/mexec.md)).
It keeps the name the file's other note gives, for its log. `kernel::program` decides
where each page of the ELF file goes and with which rights: read only,
writable or executable as its segments say, never writable and executable
at once, refused when the image does not fit above the stack or its entry
is not in its code. A program that cannot get the memory it needs does
not start, and gives back every frame it took.

**Processes.** SPAWN starts the program a file of the loader's is named
after, such as `hello.mexec`, as a process of its own, hands it a handle
the parent gives up, which becomes its argument, and answers a handle to
the process. WAIT answers how the process ended: its exit code, or the
vector of the exception that ended it. `kernel::process` keeps every
process a handle can stand for and frees it only once it ended and the
last handle to it is closed, so its parent can ask for as long as it
likes. Closing a handle closes whatever kind of object it stands for,
also when a channel closes the handles that waited in it.

**Memory for programs.** MAP gives a program fresh pages, zeroed,
writable and never executable, from 64 GiB on; UNMAP gives them back, and
a page given back faults at once. Each address is handed out once, so a
stale pointer faults instead of reaching new data.

**The runtime.** `lib/program` is what a program written in Rust starts
from: each system call as a function that answers with a `Result`, a
`main!` that gives a program its start, its panic handler and a heap of
1 MiB from MAP behind `alloc`, and `log!`, which formats a line without
the heap. Its first program is `hello`, an example of the crate, which
sextant builds into `meridian/programs`; test images carry it twice, as
`hello` and `hello_again`.

The test image runs probes in machine code that read the kernel's
memory, halt, hand the log a kernel address, an unmapped one or a line
too long, ask for a call that does not exist, write over their code, run
their stack, sleep, give way, use their stack, keep the processor for 20
ticks without giving way, set AC before a call, and read a page right
after they gave it back. It runs three programs at once, two on one core,
that each keep their own number at the same address, and checks a program
leaves no frame behind. Then it runs hello from its file, whose test build
checks every call through the runtime before it greets, makes sure hello
reached the kernel with every call, and starts it with no free frame at
all, then one more after each refusal, until it starts. Test images also
carry `exitwith`, which exits with the number it is sent, or runs out of
stack when sent anything else, for hello's checks of SPAWN and WAIT.

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

What exists: kernel threads on every core. Each core has its own run
queue, and a thread stays on the core it was started on. `lib/sched` keeps
the threads that are ready in 32 queues, one per priority, with a bit for every
queue that holds one; the highest goes first and threads of one priority
take turns. Each thread has a 32 KiB stack at `kernel::thread::STACKS`
with an unmapped page below it. A switch keeps the six registers a
function has to keep and the flags on the thread's stack, so every thread
has its own interrupt flag. A thread runs until it gives way, sleeps for a
number of timer ticks or ends, or until the next timer tick finds a thread
of the same or a higher priority ready: the tick wakes the sleepers whose
time has come and switches from inside the interrupt, and the thread that
lost the processor returns from that interrupt when it gets it back. Spin
locks keep interrupts off while they are held, so no thread loses the
processor holding one. Each core has an idle thread that halts the
processor when nothing else is ready there: on the first core one of its
own, on the others the loop that also runs their jobs. A thread started on
another core wakes it with an interrupt. Every core reads its number from
the task state it loaded, and every core's timer ticks; the first core
starts the others' by running a short thread on each, and only the first
core's ticks count the time. A thread that ends is only freed for reuse
once its core has switched away from its stack, so another core cannot
start a thread there while it is still in use. The status screen's uptime now sleeps a tenth of a second between
looks. The test image checks threads taking turns, the order of
priorities, a sleep that ends on time and not a tick late, a thread that
never gives way losing the processor at the next tick, a sleeper waking on
time while a lower thread keeps the processor busy, the guard below a
thread's stack, that every thread that can be started at once starts, ends
and frees its number, and on the other cores that a thread runs on the
core it was started on, loses the processor there at that core's tick, and
wakes there on time.

## IPC (M3)

Typed message channels that can carry handles, used by every service and by
the Windows personality.

What exists: channels between programs. A program reaches kernel objects
only through handles in its own table (`lib/handles`): a number that holds
the slot and the slot's generation, so a closed handle never reaches the
object that later takes its slot, and the rights it has, of which a
duplicate can only keep some. A channel (`lib/channel`) has two ends and a
queue of up to eight messages of up to 128 bytes each way; once one end is
closed the other reads what was sent before and then learns it is closed.
Four system calls use them: make a channel, which hands back a handle to
each end, send, receive, which leaves a message in place when its room is
too small, and close. Every address and length is checked against the
program's own address space first: what it sends has to be readable by it,
the room it receives into writable. When a program ends, every handle it
held is closed, and a channel goes once both its ends are. The test image
has a program talk to itself, two programs on different cores talk to each
other, and checks a closed end, a handle never handed out, a handle without
the right to write, a full channel, a room too small, read only memory as
room, and that no channel is left once the programs ended.

A message can carry a handle along: sending names it in r10, it needs the
right to be handed over, and it leaves the sender's table once the
message went; receiving puts it into the receiver's table, with the same
rights, and hands back its number in rdx. A handle in a message nobody
took is closed with the channel it waited in, and the channel it stood
for goes in turn once both its ends are closed. The test image has a
program hand one end of a channel of its own to a program on another
core, which takes it and reads what was sent into it, and checks a
handle nobody took leaves no channel behind.

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

What exists:

- **PCI.** The MCFG names where configuration space lies (`lib/acpi`);
  the kernel maps it uncached a page at a time in the slot for devices'
  registers and walks bus 0 and every bus behind a bridge, following a
  bridge only to a bus above its own. `lib/pci` reads each function's
  header and base address registers. Every function found goes to the
  log. Test machines have a PCIe root port with a random number generator
  behind it, which the test image wants found on bus 1.
- **AHCI.** The kernel takes the first SATA controller, turns on its
  memory and bus master bits, switches it to AHCI, and gives every port
  with a device a frame of its own for its command list, what the device
  sends back and one command table. It asks each disk who it is and reads
  sectors with READ DMA EXT, waiting for each answer; there are no
  interrupts yet. `lib/ahci` builds the commands and reads the answers,
  refusing sectors past what 48 bits address. The test image reads back
  the protective MBR and the GPT headers sextant wrote, and refuses a
  sector past the disk's end; booted from a CD, it finds the CD.
- **Partitions and FAT32.** `lib/gpt` reads a GPT header with every check
  UEFI asks for, and its entries once their CRC-32 matches; `lib/fat`
  reads FAT32 volumes, long names checked against their short entry,
  chains that may not leave the volume, loop or run past a file. The
  kernel reads every disk's partitions and each FAT32 volume on them, and
  the test image reads the EFI system partition's settings, the
  firmware's file and hello through them, hello byte for byte as the
  loader read it.
