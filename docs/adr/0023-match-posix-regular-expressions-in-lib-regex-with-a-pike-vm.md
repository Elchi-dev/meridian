# 0023. Match POSIX regular expressions in lib/regex with a Pike VM

- Status: accepted
- Date: 2026-10-27

## Context

grep finds a text as it is, which POSIX's grep does only with `-F`. Its
default is a basic regular expression (BRE), `-E` takes an extended one
(ERE), and sed and expr need the same. POSIX defines both syntaxes and
how they match: of the matches that start leftmost, the longest, and
inside it each subexpression from left to right the longest it can be
while the whole stays that match. Basic expressions also have
back-references, `\1` to `\9`, which match the text a subexpression
matched.

Meridian takes no dependencies (decision record 0003), so the engine is
written here. Engines that try alternatives one after the other and
take the first match, as most libraries outside POSIX do, find a
different match than POSIX's and can take time exponential in the
length of the text. Text in Meridian is UTF-8, and a line handed to
grep may still hold bytes that are no UTF-8.

## Decision

A crate of its own, `lib/regex`, without std, parses BRE and ERE into a
tree. A repeat with bounds is written out as copies of what it repeats,
up to a size limit past which the expression is refused. Characters are
Unicode scalar values; a byte that is no UTF-8 is a character of its own,
which only `.` and a bracket expression that leaves it out match.

Whether and where an expression matches comes from a Pike VM: the tree
compiles to a program of a few instructions, and the VM walks the text
once, keeping for each instruction only the thread that started
furthest left. Leftmost-longest falls out of that order, and the time is
the length of the text times the size of the program.

Subexpressions are worked out only when a caller asks, as sed does for
`\1` in a replacement, and only inside the match the VM found: from left
to right, each part takes the longest span with which the rest can still
match, asked of every part and span once and remembered.

An expression with back-references is no longer regular, so it is
matched by a search through the same tree that tries every way, keeping
the leftmost-longest match. That search has a budget of steps; a line
that would take more ends the match with an error rather than with
a program that seems to hang.

## Alternatives considered

- **One backtracking engine for every expression.** Simpler, and
  back-references come with it, but finding the longest match means
  trying every way, which grows exponentially with expressions as plain
  as `(a*)*b`.
- **A DFA built from the expression.** The fastest matcher, but the
  number of states can grow exponentially with the expression, and it
  says nothing about subexpressions.
- **Tagged automata that track subexpressions in one pass.** They give
  POSIX's subexpressions in linear time, but their rules for choosing
  between tags are intricate enough that published engines got them
  wrong; the span search is slower only for the subexpressions of one
  match, and is easy to check against the definition.

## Consequences

grep takes a basic expression by default, `-E` an extended one and `-F`
the text as it is, as POSIX has it, and sed and expr can be written on
the same crate. Matching is linear in the text except with
back-references, whose budget turns the worst cases into an error.
Collating elements of more than one character and equivalence classes
beyond the character itself, which need locales Meridian does not have,
are refused.
