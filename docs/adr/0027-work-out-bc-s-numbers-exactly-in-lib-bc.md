# 0027. Work out bc's numbers exactly in lib/bc

- Status: accepted
- Date: 2026-10-28

## Context

bc is POSIX's calculator: a language of its own, like awk's, over
decimal numbers of any length. Every number has a scale, the digits it
keeps after the point, and POSIX says what scale each operation gives
its result: a sum the greater of the two, a product what both have
together but no more than the greatest of `scale` and theirs, a
quotient `scale`. Digits past it are cut off, never rounded. Scripts
lean on this to the last digit, money above all.

`-l` adds a library of functions, sine, cosine, arctangent, the
exponential, the natural logarithm and Bessel's function, which POSIX
gives only by name and the scale of their results. Every bc has its
own, written in bc, and they differ in the last digits.

Meridian takes no dependencies (decision record 0003).

## Decision

A crate of its own, `lib/bc`, without std. Whole numbers of any size
are limbs of nine decimal digits, so a number's digits come out of it
without dividing, with Knuth's long division and Newton's square root.
A bc number is one of them, a sign and a scale. Each operation works
its result out exactly and then cuts it to the scale POSIX gives it,
so results match GNU bc's to the last digit, which tests check on
random numbers.

The `-l` functions are written anew in bc's language, worked out with
more digits than asked for and then cut, so their results are the true
values cut to the scale, which the tests check against values known to
more digits.

Programs are read into a tree and run, as `lib/awk` does, and what they
read and write goes through a world the bc program gives.

## Alternatives considered

- **Binary limbs.** Faster products, but every number written in base
  ten would be divided down a limb at a time, and bc writes every
  result it works out.
- **GNU's library functions.** Their code is GNU's, under the GPL, and
  their last digits are those of their own steps; the true value cut
  is what a script can count on.

## Consequences

bc's arithmetic is exact, and as fast as schoolbook products and Knuth's
division allow, which is plenty for numbers of thousands of digits. A
power whose result would pass ten million digits is refused rather than
worked out for minutes. `-l` results can differ from GNU's in the last
digit, where GNU's are off.
