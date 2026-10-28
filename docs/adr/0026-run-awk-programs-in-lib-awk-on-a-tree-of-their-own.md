# 0026. Run awk programs in lib/awk on a tree of their own

- Status: accepted
- Date: 2026-10-28

## Context

awk is the last of POSIX's classic text tools Meridian lacks, and
scripts lean on it more than on any of the others: it splits lines into
fields, keeps arrays, does arithmetic, and formats numbers and text. It
is a language, with its own grammar, values and rules. POSIX defines
it: numbers are C's doubles, a value read from the input that looks
like a number compares as one, regular expressions are extended ones,
and a number becomes a string through `CONVFMT` or `OFMT`, `%.6g` by
default.

Meridian takes no dependencies (decision record 0003), has
`lib/regex` for extended expressions (decision record 0023) and the
program runtime for files, pipes and programs, but nothing that formats
floating point numbers as C does.

## Decision

A crate of its own, `lib/awk`, without std, reads a program into a tree
and runs the tree, as `lib/sed` does for sed. The lexer knows awk's
places where a newline ends a statement and where a `/` begins a
regular expression; the parser builds statements and expressions with
POSIX's precedence, concatenation included. Values are numbers,
strings, or strings from the input that look like numbers and carry
both, and the comparisons choose between them as POSIX's table says.
Arrays are maps from strings to values.

Numbers become strings as C's printf writes them: `%d`, `%i`, `%o`,
`%x`, `%X`, `%u`, `%c`, `%e`, `%E`, `%f`, `%F`, `%g`, `%G` and `%s`, with
flags, widths and precisions, built on what Rust's `core::fmt` writes
for a double, which is exact, and turned into C's shapes.

What a program reaches beyond its variables, its input, files it reads
with `getline`, files and pipes it prints to, `system` and `close`, it
reaches through a `World` the awk program supplies, so the crate is
tested on the host with real files.

## Alternatives considered

- **A bytecode machine.** Faster on long inputs, but a second form of
  the program to keep right; a tree is as fast as awk scripts need
  until one shows otherwise.
- **awk in the shell's language.** The shell has no floating point,
  no arrays of strings and no regular expressions of the extended kind.
- **Formatting numbers by hand.** Exact decimal digits of a double take
  big-number arithmetic; `core::fmt` has it already, so only C's shapes
  are written here.

## Consequences

awk runs POSIX's programs, with its built-in functions, `getline` in its
forms, redirections to files and pipes, and user functions. `ENVIRON`
is empty, since Meridian's programs have no environment yet, and
`srand` without a seed takes the clock's ticks. Locale-dependent rules
follow the POSIX locale.

Reading and running a tree goes down it by recursion, so a program's
depth is what its stack holds. awk asks for the most a program may, 1
MiB (decision record 0022), and lib/awk is built with optimisations
even where the rest is not, since unoptimised each level takes several
times the stack. Expressions and statements nest 100 deep at most, an
expression joins 1000 operations in a row at most, and calls go 500
deep at most; past them a program is refused, or stops, with a message
rather than a fault.
