# The shell language

Draft, version 0

Meridian's shell reads two dialects (decision record 0016). This document
describes Meridian's own, the one the shell reads unless told otherwise.
The sh dialect is POSIX sh, as the Shell Command Language of IEEE Std
1003.1 describes it, and is not repeated here.

```
# Copy every text file into a folder of its own.
let folder = /System/Notes
md $folder
for file in *.txt {
    cp $file $folder
    echo "copied $file"
}
if cat $folder/todo.txt > /System/last.txt and not ls /System/Old {
    echo done
} else {
    echo "something went wrong"
}
```

A conforming reader turns exactly the scripts described here into
commands and refuses every other one at the line and column this
document gives, with its error code.

## 1. Characters and lines

A script is UTF-8. Lines end with a line feed, or a carriage return
followed by a line feed. Positions are a line and a column from 1, the
column counting Unicode scalar values, as in `.meri`. A carriage return
without a line feed after it is an error (`lone-carriage-return`).

Whitespace is the space and the tab. A `#` outside quotes, at the start
of a word, starts a comment to the end of the line; inside a word it is
an ordinary character, so `a#b` is one word.

## 2. Words

A word is a run of characters up to whitespace, a line's end or one of
the operators `|`, `||`, `&&`, `;`, `>`, `<`, `{` and `}`. A single `&`
is an ordinary character. These make up a word:

- Plain characters stand for themselves.
- `'...'` holds characters as they are, `$` and `\` included, up to the
  next `'` on the same line.
- `"..."` holds characters up to the next unescaped `"` on the same line,
  with `$name` and `${name}` replaced by the variable's value, and `\"`,
  `\\`, `\$`, `\n` and `\t` standing for `"`, `\`, `$`, a line feed and a
  tab.
- `$name` and `${name}` outside quotes are replaced by the variable's
  value. A name is a letter or `_` and then letters, digits and `_`.
  `$?`, `$#` and `$1`, `$2` and on are the special ones of section 5 and
  6. `${` that does not hold a name and then `}` is an error
  (`bad-variable`); a `$` that starts no variable is a `$`.
- `\` outside quotes takes the next character as it is.

Parts follow each other without space to make one word: `"$dir"/file`
and `$dir/file` are both one word. A variable's value is always one word
or part of one; it is never split at spaces and never matched against
file names. A variable that was never set is an error
(`unset-variable`), reported at its `$`.

A word whose plain parts hold `*` or `?` is a pattern: it stands for the
names of the files it matches, in the order of their names compared in
capitals, as many words. A pattern that matches nothing is an error
(`no-match`). Characters in quotes or from a variable never make a word a
pattern.

A quote that is not closed on its line is an error (`unterminated-quote`),
reported at the opening quote. A `\` at the end of a line outside quotes
joins the next line to this one.

## 3. Commands

A command is one or more words, the first of which names it: a builtin
of the shell, a program by its name in `/System/Boot/meridian/programs`,
or where a CD holds no such directory, among the programs the loader
brought, or a program by its path. The other words are its arguments.

Redirections may stand anywhere among a command's words:

| Redirection | Meaning |
| --- | --- |
| `> PATH` | what the command writes goes into the file, made new or emptied |
| `>> PATH` | what the command writes goes after what the file holds, making it when it is not there |
| `< PATH` | the command reads the file instead of the keyboard |
| `2> PATH` | what the command says went wrong goes into the file, made new or emptied |
| `2>> PATH` | what the command says went wrong goes after what the file holds |
| `2>&1` | what the command says went wrong goes where its output goes at that point, a file or the next command of a pipeline |

The path is the next word. A redirection without one is an error
(`missing-path`). A `2` alone right before `>` or `>>`, with nothing
between them, makes them `2>` and `2>>`; in a word, as in `a2>f`, or
apart from them, it is an ordinary word.

A `{` where no block starts is an error (`unexpected-open`); quoted, it
is part of a word.

Commands are joined into a pipeline with `|`: what each one writes, the
next one reads. A pipeline's exit status is that of its last command. A
`|` with no command on either side is an error (`missing-command`).

## 4. Lists

Pipelines run one after another, separated by `;` or a line's end. A
statement ends there or at the `}` of its block; anything else after it
on its line is an error (`expected-end`).
`and` between two pipelines runs the second only when the first ended
with 0; `or` runs it only when the first did not. `&&` and `||` are
`and` and `or` as sh writes them. `not` before a pipeline
turns 0 into 1 and anything else into 0. `and` and `or` bind equally and
from the left, so `a or b and c` runs `c` once `a or b` succeeded. `not`
has this meaning where a pipeline starts, and `and` and `or` right after
a command's first word or any later one, so `echo and` waits for a
command; anywhere else, and quoted, the three are ordinary words.

## 5. Variables

`let name = value` sets a variable; the value is one word. `let` without
a name, without `=` or without a value is an error (`expected-name`,
`expected-equals`, `expected-value`).
A variable lives until the script or session ends, or until the block it
was set in ends when it was not set before that block.

`$?` is the exit status of the last pipeline. A `for` loop's variable
belongs to its block, as if set there, and so does one set by `let`
inside a block that was not set outside it; setting one that was set
outside changes that one.

## 6. Blocks

`if`, `else`, `for`, `while` and `fn` take a block: `{`, lists, `}`. The
`{` is on the same line as its keyword, or it is an error
(`expected-brace`). A block may stay on one line: `if ls /x { echo yes }`.
`for` and `fn` without a name are an error (`expected-name`), and `for
NAME` without `in` after it is one too (`expected-in`).

- `if LIST { ... }` runs the block when the list ended with 0. An `else
  { ... }` or `else if ...` may follow right after its `}`.
- `for name in WORDS { ... }` runs the block once for each word, with the
  variable set to it.
- `while LIST { ... }` runs the block as long as the list ends with 0.
- `fn name { ... }` makes a command of its own. Its arguments are `$1`,
  `$2` and so on, and `$#` says how many there are.

`break` and `continue` work in `for` and `while`, the innermost one;
anywhere else they end the script. `return` ends a function, with the
status it is given, a number from 0 to 255, or with 0; any other word is
an error (`bad-status`). Outside a function it ends the script the same
way. `exit` ends the script, or the session, from inside any block, loop
or function, with the status it is given in the same way, or with that
of the last pipeline.

`true` ends with 0 and `false` with 1, without running anything. A name a
function was made with is a command from then on, before any program of
that name. Functions may call each other 64 deep; one more is an error
(`too-deep`).

A `}` without a block is an error (`unexpected-close`); a block still
open at the end of a script is an error (`unclosed-block`), reported at
its keyword. `else` that does not follow an `if` block's `}` is an error
(`unexpected-else`).

## 7. Builtins

`cls` and `clear` clear the console. The others are listed by `help`.
Every command answers to its Linux name and to its Windows one where
there is one: `ls` and `dir`, `cat` and `type`, `cp` and `copy`, `mv` and
`move`, `rm` and `del`, `mkdir` and `md`, `rmdir` and `rd`.

## 8. Errors

Reading stops at the first error, reported as `LINE:COLUMN: code: what
to do`, for example `3:9: unterminated-quote: close the " that starts
here on the same line`. The codes are the ones this document names. Running a
command that does not exist is not a reading error; it ends that command
with status 127 and says so.

## 9. The sh dialect

The sh dialect reads scripts as POSIX sh does, and reports its errors in
the same form. A quote, a `${`, a `$(`, a `$((` or a backquote that the
script leaves open is an error (`unclosed`), reported where it opens.
Quotes and expansions in sh may go on over several lines, so the error
comes only at the script's end. Inside a `$(`, a `${` or a `$((` the
error is at the last thing opened, which is the first to close.

Expansions may stand inside each other 16 deep, as in `$(a "$(b)")`,
which is 2 deep. One deeper is an error (`too-nested`), reported where it
opens: reading each one takes some of the shell's stack, and a script
that needs more can set a variable to an inner one first.
