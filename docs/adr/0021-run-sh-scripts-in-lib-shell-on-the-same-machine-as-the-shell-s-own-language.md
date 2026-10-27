# 0021. Run sh scripts in lib/shell on the same machine as the shell's own language

- Status: accepted
- Date: 2026-10-27

## Context

Decision record 0016 gives the shell a POSIX sh dialect next to its own
language, both reading into the same commands. lib/shell now reads sh
into its tokens and commands (`sh::lex`, `sh::parse`), matches its
patterns (`sh::pattern`) and works out its arithmetic (`sh::arith`). It
does not run them yet.

lib/shell runs the shell's own language in a `Session` on a `Machine`, a
trait with two calls: run a pipeline of ready commands, each its words
and redirections, and list the files a pattern matches. The shell
program implements it: its commands (`echo`, `ls`, `cat`, `cp`, `mv`,
`rm`, `cd` and the rest) run in its own threads when they stand in a
pipeline, and programs run through SPAWN with the three streams of
decision record 0017. lib/shell's tests run on a machine with no
programs and no files, so they cannot see what a command writes.

What sh asks for beyond that:

- `$(...)` takes what commands write, compound ones too, as text. A
  program writing more than a channel holds waits until it is read, so
  whatever takes the text has to read while the commands run.
- Redirections name streams by number, 0 to 9, copy one onto another
  with `>&` and `<&`, close one with `-`, and a here-document makes a
  text a command's input. Meridian's programs have three streams.
- Subshells, `( ... )` and `$(...)`, run in a copy of the shell: what
  they set, `cd` among it, does not come back.
- `&` starts a command without waiting, `$!` names it, `wait` waits.
- `trap` takes signals, which Meridian does not send.
- `echo`, `printf`, `test` and `[` are commands every sh script uses,
  and every shell has them built in.
- Patterns are matched as POSIX says, capitals as other letters, while
  Meridian's volumes compare names in capitals.
- `export` hands variables to the programs a shell starts, which SPAWN
  does not carry yet.

## Decision

The sh dialect runs in lib/shell, in a session of its own beside the
language's, on the same `Machine`. The machine grows where sh needs it,
and both dialects share what grows:

- A command's redirections name stream 0, 1 or 2 and what it becomes: a
  file opened to read, to write new, to write at its end or to read and
  write, a copy of another of the three, nothing, or a text, for a
  here-document. A number above 2 is refused when the command runs, with
  an error that says Meridian's programs have three streams.
- The machine takes what the commands it runs write without a file for
  it into a text between `begin_capture` and `end_capture`, which nest.
  The shell program reads the capture's channel in a thread of its own
  while the commands run.
- `echo`, `printf`, `test`, `[`, `true`, `false` and `:` are written
  once in lib/shell, as functions of their words, their input and their
  output. The shell program calls them for both dialects, in a thread
  when they stand in a pipeline, as its own commands run now, and
  lib/shell's tests call them on a machine that keeps what is written.
- A subshell runs in a copy of the session, and the working directory,
  which is the runtime's for the whole program (decision record 0020),
  is set back when it ends.
- `&` starts a pipeline without waiting for it. `$!` and `wait` name and
  wait for its last program; a command of the shell's own in the
  background runs in a thread.
- `trap` takes `EXIT` and `0`, run when the shell ends; any other
  condition is refused with a word that Meridian sends no signals.
- Patterns match as POSIX says, in `case`, in `${name%pattern}` and in
  the names of files alike: portable scripts count on capitals being
  other letters.
- `export` marks a variable. Programs get exported variables once SPAWN
  carries an environment, which a decision record of its own adds.

## Alternatives considered

- **Run sh in the shell program, not lib/shell.** It holds the threads
  and the streams, but its tests run only in QEMU, and the language's
  run would have to move there too to share what both dialects need.
- **Keep echo and the like in the shell program alone.** lib/shell's
  tests then cannot run `$(echo x)`, and a stand-in for echo on the test
  machine would be a second echo that only tests use.
- **Ten streams for every program.** The runtime and every program would
  carry seven streams nothing on Meridian uses; scripts that copy stream
  3 are rare and get a clear error.
- **Subshells as processes, as POSIX shells fork.** Meridian has no fork,
  and starting the shell again for each `( ... )` and `$(...)` would
  have to hand over every variable and function.
- **Patterns that compare in capitals, as Meridian's volumes do.** Names
  would match better on Meridian, but scripts written for Linux and
  macOS would match differently there than where they were written.

## Consequences

lib/shell's tests can run sh scripts end to end, `$(...)` and pipelines
of the shared commands included, without QEMU. The shell program's
`echo` moves into lib/shell, so the language's echo and sh's are one.
The machine gets three more calls and a richer redirection, which the
shell program has to implement and check in QEMU. Scripts that need
streams above 2, signals other than EXIT, or exported variables in the
programs they start get an error or nothing until Meridian has them; the
specification says which.
