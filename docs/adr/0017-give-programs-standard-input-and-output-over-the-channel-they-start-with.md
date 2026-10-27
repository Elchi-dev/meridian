# 0017. Give programs standard input and output over the channel they start with

- Status: accepted
- Date: 2026-10-27

## Context

The shell language of decision record 0016 has pipes and redirections:
`ls | grep a`, `cat a > b`. Today a program writes text with WRITE,
which goes straight to the console, and reads keys with KEY, which come
straight from the keyboard. The shell has no way to send what a program
writes into a file or into another program.

SPAWN hands the program it starts one handle, which `main` receives;
init uses it for the line each program asks it about services over.
Channels carry messages of up to 128 bytes, and a message can carry a
handle with it. A file opened to write takes WRITE_FILE.

Every program Meridian will run expects standard streams: Windows gives
each process a standard input, output and error handle in its start
parameters, which `GetStdHandle` hands out and `WriteFile` writes to,
and POSIX gives file descriptors 0, 1 and 2. The Win32 layer of M6 and a
POSIX layer later both need them.

## Decision

The handle a program starts with is its start channel. Before the
program's own code runs, the runtime reads what its parent sent there,
each a message with one kind byte and, for most kinds, a handle:

| Kind | What it is |
| --- | --- |
| 1 | standard input, a channel end to read from |
| 2 | standard output, a channel end to write to |
| 3 | standard error, a channel end to write to |
| 4 | standard output, a file opened to write |
| 5 | standard error, a file opened to write |
| 6 | init's line, to ask about services over; without a handle, the start channel itself |
| 0 | the end of what the parent sends, with no handle |

Text on a channel goes as messages of up to 128 bytes, and an empty
message ends the stream. A writer whose channel is full waits until the
other end took a message. Writing to a file goes on at the file's end.

A program without a standard output writes to the console as now, and
one without a standard input reads keys from the keyboard. The runtime
answers both through the same calls, so a program does not need to know
where its text goes. A program the kernel starts, init alone, gets no
start channel.

The shell joins `a | b` with one channel, the write end as `a`'s
standard output and the read end as `b`'s standard input, opens the file
of `> file` itself and hands it on, and reads a channel for its own
builtins.

## Alternatives considered

- **A pipe object in the kernel.** Windows and POSIX both have one, but
  channels already move bytes between programs with backpressure and
  carry handles; a second kind of object would duplicate them.
- **More handles in SPAWN's registers.** Four registers are taken by the
  name, its length, the handle and the restriction, and the list of what
  a program may need will grow: a working directory, an environment.
- **A table of start values in the program's memory.** It needs a new
  agreement between the kernel and every program about where the table
  lies, where a channel needs none.

## Consequences

The kernel does not change: it already starts programs with a handle and
moves messages. The runtime grows the start messages, the streams on
channels and files, and keeps the console for programs without them.
init starts each program with its line and says so in a start message
of kind 6 without a handle, so the line goes on carrying the questions
about services once the start messages are read, and the shell gets the
line from what the runtime kept. Each start costs a few messages
before `main`. Environment variables and a working directory, which 0016
and Win32 need next, become two more kinds of start message.
