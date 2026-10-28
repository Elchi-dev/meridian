# 0024. Run the shell's shared commands as programs of their own too

- Status: accepted
- Date: 2026-10-28

## Context

Decision record 0021 put `echo`, `printf`, `test` and `[`, `true`,
`false` and `:` in lib/shell's `commands`, which both of the shell's
dialects run in the shell itself. Nothing else can run them: a program
that starts another program does so by its file in
`/System/Boot/meridian/programs`, and these commands have none.

POSIX names them utilities as well as built-ins. `xargs` runs `echo`
when it is given no utility, and runs whatever it is given as a program;
`find -exec`, `env` and `nohup` do the same. On Unix systems each of
them is a file too, `/bin/echo`, `/usr/bin/printf`, `/usr/bin/test`,
`/bin/true` and `/bin/false`, next to the shell's own.

`commands::run` takes the words and a `Streams`: where the command
writes, where it says what went wrong, what a file is and whether a
stream is a terminal. The shell program implements it on its own
streams and files. A program implements it the same way on the streams
it was handed at its start (decision record 0017).

## Decision

`echo`, `printf`, `test`, `true` and `false` are programs on every
image, each running `commands::run` with its name and its arguments on
the streams it was handed. What a file is, and what the one asking may
do with it, moves from the shell program into the program crate's
`commands` module, which both use, so `test -r` answers the same in the
shell and as a program.

`[` and `:` stay built-ins only: a program's file is named after a
Cargo example, which cannot be `[`, and `:` does nothing a program would
be started for.

The shell keeps running its own copies: a command it knows runs in the
shell, as before, and the programs only serve what starts programs.

## Alternatives considered

- Have xargs and the like start the shell with a script that runs the
  command. Every word would have to be quoted for the shell's language,
  each run would start a whole shell, and the shell's other commands,
  `rm` among them, would be reached as well, which POSIX's utilities do
  not reach.
- Give each tool that starts programs its own echo. That only covers
  echo, and every such tool would carry a copy.

## Consequences

Five more programs, and their second copies on test images. A change to
a shared command reaches the shell and the program alike, since both run
lib/shell's code. The shell's tests of these commands still run on the
host; the programs get a check of their own in the test image.
