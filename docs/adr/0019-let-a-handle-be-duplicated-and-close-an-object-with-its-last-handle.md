# 0019. Let a handle be duplicated, and close an object with its last handle

- Status: accepted
- Date: 2026-10-27

## Context

A program hands a handle to another by sending it in a message, which
moves it: once SEND took it, the sender holds it no more. Each handle
stands for its own channel end, open file or process, and closing it
closes the end or the file, so two handles to one end cannot exist.

The shell of decision record 0016 runs with a standard output (decision
record 0017), and every program it starts without a redirection should
write there too, while the shell keeps writing there itself. Today such a
program gets no standard output and writes to the console, so what
`shell -c` hands back is only what the shell wrote, not its programs.

Windows gives a process a second handle to the same object with
`DuplicateHandle`, and lets a child inherit handles at its start; the
object goes once its last handle closed. POSIX has `dup`, and a pipe's
reader sees the end of the stream once every descriptor of its write end
is closed. Processes in Meridian already count the handles that stand for
them, so a process outlives none of its handles.

## Decision

DUPLICATE (26) takes a handle and answers a new one in the same program,
to the same object, with the same rights. It can do nothing the first
one could not, so it needs no right of its own.

Channel ends and open files count their handles as processes do, those
in messages still waiting included. Closing a handle takes one away, and
the object closes with its last one: the other end of a channel reads as
closed once no handle to its end is left, and an open file goes once
none stands for it.

## Alternatives considered

- **Hand over the original and keep none.** The shell would lose its own
  output to the first program it starts.
- **A second object instead of a second handle.** A second channel needs
  someone to pass the messages from one to the other, a thread for every
  program, and a channel end cannot be opened again the way a file can.
- **Inheritance at SPAWN, as `bInheritHandles`.** SPAWN's registers are
  taken, and decision record 0017 already hands a program what it starts
  with over its start channel; a duplicate goes there like any handle.

## Consequences

The runtime offers `program::duplicate`, and the shell hands each program
without a redirection a duplicate of its own standard output, so a parent
sees what the shell's programs write as well. Every kind of object keeps
a count, and every new kind will have to. A duplicate left open keeps the
other end of its channel from ever reading closed, as a forgotten
descriptor keeps a pipe open on POSIX, so programs close what they no
longer need. Duplicating with fewer rights, as `DuplicateHandle` can,
waits for the Win32 layer of M6.
