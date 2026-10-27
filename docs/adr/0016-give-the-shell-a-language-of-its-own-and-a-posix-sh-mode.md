# 0016. Give the shell a language of its own and a POSIX sh mode

- Status: accepted
- Date: 2026-10-27

## Context

QUESTIONS.md 12.1 asks for Meridian's own shell language that also runs
sh, switched by an environment variable or something like it, with `cls`
and `clear` both working always. 12.2 asks for the command names an
average person who opens a terminal finds easiest. 12.4 asks that a
normal user never has to fear the system.

The shell today reads one line as a command word and the rest: `help`,
`echo`, `ls`, `cat`, `mkdir`, `rm`, `clear` and `cls`, `services`,
`exit`, `echo TEXT > PATH`, and programs by name or path. It has no
variables, quoting, pipes or control flow. Processes carry no
environment yet.

What people bring with them is sh: shell scripts from Linux and macOS,
answers on the web, and habits like `ls | grep`. What trips them up in sh
is just as well known: a variable splits into several words unless it is
quoted, a pattern that matches nothing is passed on as it is, `[` is a
program with its own rules for spaces, and an error says little more
than a line number.

## Decision

One program, the shell, reads two dialects into the same commands: words,
quoting, variables, pipes, redirections, exit codes, and running
programs. Its own dialect is the default. The sh dialect is POSIX sh as
the Shell Command Language of IEEE Std 1003.1 describes it.

Meridian's dialect keeps what sh users type all day: commands and their
words, `"` and `'` quotes, `$name`, `|`, `>`, `>>` and `<`, `;` and a
line's end between commands, `#` comments. It drops sh's traps and writes
control flow in the blocks `.meri` already uses:

- `let name = value` sets a variable; `$name` is always one word, never
  split, and a pattern such as `*.txt` that matches nothing is an error
  rather than the pattern itself.
- `if`, `else`, `for name in ...`, `while` and `fn name` take a block in
  braces; conditions are commands, as in sh, joined with `and`, `or` and
  `not`.
- An error names the line, the column and what to do, in plain words,
  like the errors of `.meri`.

The dialect is chosen per script and per session: a script whose first
line is `#!/bin/sh` or `#!sh` is read as sh; the shell started under the
name `sh` reads sh; the `sh` command switches a session to sh and `exit`
comes back; and once processes carry an environment, `SHELL_DIALECT=sh`
makes sh the default for a user.

Commands keep their Linux names, `ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`,
and answer to the Windows ones as well, `dir`, `type`, `copy`, `move`,
`del`, `md`, `rd`, `cls`, in both dialects, so what someone already knows
works wherever they come from.

## Alternatives considered

- **sh alone.** Every script and habit carries over, but the traps above
  stay, and the language cannot become friendlier without stopping being
  sh.
- **A new language alone.** Free to be as friendly as possible, but no
  script from elsewhere runs, and 12.1 asks for sh too.
- **Structured pipelines that pass objects, like PowerShell or Nushell.**
  Powerful, but each command needs to know the objects, nothing outside
  speaks it, and 12.1 left it out.
- **sh syntax with extras.** Keeping sh's grammar and adding to it keeps
  the traps, since scripts written for sh rely on word splitting.

## Consequences

Two grammars have to stay in step on one core. The dialect gets a written
specification in `docs/formats/shell.md` with a suite of scripts and
their output, as `.meri` has, and the sh dialect is checked against what
the POSIX standard says, read from the standard itself. Processes need an
environment before `SHELL_DIALECT` means anything, which SPAWN does not
carry yet. The Windows command names are aliases of the same commands,
so their options are Meridian's, not cmd.exe's.
