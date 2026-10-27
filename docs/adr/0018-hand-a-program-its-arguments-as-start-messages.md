# 0018. Hand a program its arguments as start messages

- Status: accepted
- Date: 2026-10-27

## Context

The shell of decision record 0016 runs `cp a b`, `ls /System` and
`echo hi`: a program by its name, and words after it that the program
needs. SPAWN takes the program's path, one handle and a restriction, and
nothing else; a program today learns nothing but what its parent sends
it later over that handle.

Decision record 0017 made that handle the start channel, over which the
parent sends what the program starts with before its own code runs, each
a message of one kind byte. Messages are at most 128 bytes long, and an
argument can be as long as a path, 512 bytes, or longer.

Windows hands a process one command line, which each program splits into
words by rules of its own, and quoting breaks between programs that split
differently. POSIX hands each argument on its own, as `argv`.

## Decision

The shell splits a command into words once, by the rules of its
language, and each program gets those words as they are. They go as
start messages after the kind byte, in the order they have:

| Kind | What it is |
| --- | --- |
| 7 | an argument: the bytes after the kind, UTF-8 |
| 8 | more of the last argument, for one longer than a message holds |

Neither brings a handle. A message of kind 7 with nothing after the kind
is an empty argument, which a program can be given. The program's own
name is not among them. The runtime keeps the arguments in order, and
`program::arguments` hands them out.

When the Win32 layer of M6 starts a program, it joins the words into one
command line as Windows programs expect to split it, so they get the same
words.

## Alternatives considered

- **One command line, as Windows has it.** Every Meridian program would
  have to split it again, and would split it differently from the shell.
- **The words in SPAWN, in the parent's memory.** SPAWN's registers are
  taken, and the kernel would copy and keep words it has no use for.
- **A file of arguments.** It needs a place to write it and to clean it
  up after, where a message is gone once read.

## Consequences

A program with many long arguments costs one message for every 127 bytes
of them before its code runs. Kinds 7 and 8 are the first start messages
longer than one byte, so the runtime reads the bytes after the kind
for them alone. Environment variables, which 0016's `SHELL_DIALECT`
needs, can follow the same way as their own kind.
