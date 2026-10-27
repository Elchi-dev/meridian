# 0020. Keep a program's working directory in its runtime, handed over at its start

- Status: accepted
- Date: 2026-10-27

## Context

Every path Meridian's calls take starts at the root: the VFS refuses
`a.txt` as no path. The shell of decision record 0016 keeps a directory
in its session, which patterns that do not start at the root start
from, but has no `cd`, so people type `/System/...` for every file.
Every program Meridian runs expects to have a current directory: Windows
keeps one for each process, and POSIX does too.

On Windows the current directory lives in user mode, in the process's
parameters, and the Win32 layer turns a relative path into a full one
before it calls the kernel, which takes full paths only. A child starts
in its parent's directory unless the parent names another. POSIX keeps
it in the kernel, where `chdir` changes it.

Decision record 0017 hands a program what it starts with over its start
channel, and 0018 hands it text there, with kind 8 for more of a text
than one message holds.

## Decision

A program's working directory is its runtime's, as Windows has it. Its
parent hands it over as a start message of kind 9, the path after the
kind byte in UTF-8, with kind 8 for more of it as for an argument; a
program handed none starts at the root. The runtime keeps it, and
`program::directory` answers it.

Each call of the runtime that takes a path turns one that does not start
at `/` into one that does, by putting the working directory before it,
so the kernel goes on taking paths from the root alone. The runtime
changes the directory only when asked to, and only to one that is
there.

The shell's `cd` changes its own, and each program it starts gets the
shell's directory.

## Alternatives considered

- **The directory in the kernel, as POSIX keeps it.** Every call with a
  path would need the process's directory, and the Win32 layer of M6
  keeps the one Windows programs see in user mode anyway.
- **No working directory, paths from the root only.** What people type
  at a shell and what every program expects both need one.
- **The shell alone turns its words into full paths.** A program would
  still get `a.txt` as an argument and could not open it.

## Consequences

Programs started with a relative path for an argument open the file
their user meant. Two threads of one program share its directory. A
directory removed while it is a program's leaves the program in a
directory that is not there, as on Windows and POSIX; paths under it
then are not found. The Win32 layer can keep `GetCurrentDirectory` in
the same place.
