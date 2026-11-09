# 0032. Convert character sets in lib/charset, from the tables Unicode publishes

- Status: accepted
- Date: 2026-11-09

## Context

Meridian's programs read and write UTF-8. POSIX's `iconv` converts text
between character sets, and the Windows programs Meridian is to run
speak UTF-16 and the code pages of their time: `MultiByteToWideChar`
and `WideCharToMultiByte` take Windows-1252 and the PC's code page 437
among others, and a console shows 437's box drawing. Both need the same
tables and the same handling of what does not convert.

Unicode publishes Microsoft's tables for the code pages, byte by byte,
in its mappings directory, and glibc's iconv converts the same sets, so
a table can be checked against it on the host.

## Decision

`lib/charset` turns bytes into Unicode scalar values and back, a piece
at a time, for UTF-8, UTF-16 and UTF-32 with and without a byte order
mark, US-ASCII, ISO 8859-1, Windows-1252 and code page 437. A byte or a
sequence that stands for no character, or a character a set has no
bytes for, is answered as such for the caller to stop at or leave out.
UTF-16 and UTF-32 named without an order are read in the order their
mark gives, or the machine's, little-endian, without one, and written
little-endian after a mark, as glibc does. The tables of the 8-bit sets
are written out from Unicode's files, and the tests compare every byte
of each with glibc's iconv. `iconv` is built on it, and the Win32
personality will be.

## Alternatives considered

- **UTF-8 alone in iconv**, the rest later. The code pages are what
  Windows programs bring, so the tables are needed either way.
- **Tables made at build time from Unicode's files.** That needs the
  files in the repository and a build step for 256 numbers a set.
- **ICU's names and its many sets.** Each set costs a table and its
  checks; more can join the same crate as programs need them.

## Consequences

`iconv` converts among the sets listed, and the Win32 code-page
functions start from the same tables. A set added later is a table
and an entry in the list of names.
