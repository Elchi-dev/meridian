# 0025. Find differences in lib/diff with Myers' algorithm in linear space

- Status: accepted
- Date: 2026-10-28

## Context

POSIX's diff says how to make one file of another: which lines to leave
out of the first and which to put in. Any such list is correct, but the
tools people know write a short one, the fewest lines changed, and
programs such as patch and ed take what diff writes as instructions, so
its forms have to be exact. POSIX defines the forms in outline; where it
leaves room, as for a last line without a line feed or for how near two
changes must lie to share their lines of context, GNU's diff is what
scripts and people expect.

Meridian takes no dependencies (decision record 0003). A program has a
heap of 1 MiB, so a way of comparing that keeps a table for every pair
of lines, or every step it took, runs out of room on files of a few
thousand lines.

## Decision

A crate of its own, `lib/diff`, without std, finds the lines two texts
have in common and writes the differences in each of diff's forms. The
program turns each line into a number first, the same for lines that
compare the same (with `-b`, runs of blanks as one), so the comparing
compares numbers.

The lines in common come from Myers' O(ND) algorithm in its linear space
form. The lines both texts start and end with are taken first; then the
shortest way through what is left is cut in two where the ways from both
ends meet, and each half is compared the same way, from a list of tasks
rather than by recursion, so a long text needs no deep stack. The result
is the most lines in common there can be, found in time that grows with
the length of the texts times the number of lines that differ, in room
that grows with their length alone.

The forms follow GNU's diff in what POSIX leaves open, and the tests
hold them to its output: the note `\ No newline at end of file`, hunks
merged when their context lines meet, and ed scripts that put a line of
one dot in as two and put it right afterwards. For two files that are no
text, diff says `Files a and b differ` as POSIX words it.

## Alternatives considered

- **The table of every pair of lines.** The plainest way to the most
  lines in common, and the tests use it as the reference, but its room
  grows with the product of the lengths.
- **Hunt and McIlroy's algorithm, the first diff's.** Fast when few lines
  repeat, but slow and large when many do, as blank lines and braces do
  in most files.
- **Patience or histogram diff.** They give changes that read well in
  code, but not the fewest lines changed, and so not what diff is known
  to write.

## Consequences

diff finds the fewest lines changed, and files that differ little cost
little. Both files are held whole, so the heap a program has bounds the
size of the files it compares. Two long files with nothing in common are
the slowest case, quadratic in their length; GNU's diff gives up on the
shortest way there with a heuristic, which can be added if it is needed.
The headers of `-c` and `-u` name the files without a time, since
Meridian's files keep none yet.
