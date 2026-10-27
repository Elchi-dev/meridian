# 0022. Let a program ask for a bigger stack in a note of its file

- Status: accepted
- Date: 2026-10-27

## Context

The kernel gives every program 16 pages of stack, 64 KiB, mapped when it
starts, with an unmapped page below them, so a program that runs past
them faults with a page fault and ends. Nothing grows the stack later.

The shell's sh dialect (decision record 0021) runs a script by
recursion: each compound command, function call, `eval` and `$(...)`
running inside another takes stack. Measured on the host, one level
takes 2 to 3.6 KiB optimised and 3 to 7 KiB unoptimised, which is how
the QEMU test image builds its programs. Sixteen nested function calls,
which the dialect allows, took 79 KiB optimised, and the shell faulted
on them in QEMU. POSIX shells recurse as deep as the stack lets them,
and scripts nest a few levels each way, so 64 KiB is too little for an
interpreter while it is plenty for most programs.

A system call checks that a buffer a program hands it is mapped before
it copies, so a stack that grows on a page fault would also have to grow
inside those checks, for buffers on stack pages not touched yet.

## Decision

A program may ask for more stack in a note of its `.mexec` file: kind 3,
`NOTE_STACK`, whose four bytes say how many pages it wants. The kernel
maps that many when it starts the program, as it maps 16 now, with the
unmapped page below them. Asking for fewer than 16 gives 16, and asking
for more than 256, 1 MiB, refuses the file, as a version the kernel does
not know does. An image must fit above its own stack, so a program
asking for more leaves less room for its code.

`main!` in lib/program writes the note when a program names its pages:
`program::main!(shell, stack = 256)`. The shell asks for 256 pages, and
the sh dialect limits how deep commands may run inside each other so
that the deepest script fits in them.

## Alternatives considered

- **Grow the stack on a page fault, up to a limit.** It costs memory
  only for the pages used, but every system call that checks a buffer
  would have to grow the stack first when the buffer lies below it, and
  a program's other threads would race for the same pages.
- **Give every program 1 MiB.** Every process would take 1 MiB of memory
  at its start for a stack almost none of them need.
- **Run the shell's sessions in a thread with a bigger stack.** Threads
  get 8 pages each, and giving them a size of their own is the same
  change in a place programs reach less directly.
- **Use the linker's `PT_GNU_STACK` size.** Cargo sets link arguments for
  all examples of a package at once, not for one program, and a note is
  how a `.mexec` file already says what it is.

## Consequences

A program states the stack it needs where its name is, and the kernel
gives it at the start, so running past it still faults at once as it
does now. Programs that recurse, the shell first, can be written as
recursion. The images of those programs get less room: 3 MiB less 4 KiB
with 1 MiB of stack. Memory for a stack is taken whole at the start, so
a program asking for much costs that much while it runs.
